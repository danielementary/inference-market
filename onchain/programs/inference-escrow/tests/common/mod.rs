#![allow(dead_code)]
// `TransactionResult` is LiteSVM's type; its large `Err` variant is not ours to box.
#![allow(clippy::result_large_err)]

use {
    anchor_lang::{
        prelude::Pubkey,
        solana_program::{bpf_loader_upgradeable, instruction::Instruction, system_program},
        AccountDeserialize, AccountSerialize, InstructionData, ToAccountMetas,
    },
    inference_escrow::{accounts, instruction, Config, Model, Request, Worker},
    litesvm::{types::TransactionResult, LiteSVM},
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
};

pub const FEE_BPS: u16 = 200;
pub const REQUEST_FEE: u64 = 10_000;
pub const BUDGET: u64 = 1_000_000;
pub const NONCE: u64 = 1;
pub const PROMPT_HASH: [u8; 32] = [7; 32];

/// Worker 1 matches the default request; worker 2 serves another model.
pub const W1: Offer = Offer {
    model: Model::Qwen38,
    tok_per_s: 40,
    price_in: 100,
    price_out: 400,
};
pub const W2: Offer = Offer {
    model: Model::Llama4,
    tok_per_s: 20,
    price_in: 50,
    price_out: 200,
};

#[derive(Clone, Copy)]
pub struct Offer {
    pub model: Model,
    pub tok_per_s: u32,
    pub price_in: u64,
    pub price_out: u64,
}

/// Terms of the default request: Qwen38, at least 30 tok/s, prices that accept worker 1.
#[derive(Clone, Copy)]
pub struct Terms {
    pub model: Model,
    pub min_tps: u32,
    pub max_price_in: u64,
    pub max_price_out: u64,
    pub budget: u64,
}

pub const TERMS: Terms = Terms {
    model: Model::Qwen38,
    min_tps: 30,
    max_price_in: 150,
    max_price_out: 500,
    budget: BUDGET,
};

pub struct Env {
    pub svm: LiteSVM,
    /// Pays every transaction fee, so the other parties' balances only move through the program.
    pub payer: Keypair,
    pub admin: Keypair,
    pub coordinator: Keypair,
    pub client: Keypair,
    pub owner1: Keypair,
    pub owner2: Keypair,
    pub attacker: Keypair,
}

pub fn program_id() -> Pubkey {
    inference_escrow::id()
}

pub fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[Config::SEED], &program_id()).0
}

pub fn worker_pda(owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[Worker::SEED, owner.as_ref()], &program_id()).0
}

pub fn request_pda(client: &Pubkey, nonce: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[Request::SEED, client.as_ref(), &nonce.to_le_bytes()],
        &program_id(),
    )
    .0
}

fn program_data_pda() -> Pubkey {
    Pubkey::find_program_address(&[program_id().as_ref()], &bpf_loader_upgradeable::ID).0
}

fn event_authority() -> Pubkey {
    inference_escrow::EVENT_AUTHORITY_AND_BUMP.0
}

/// Fresh VM with the program loaded, `admin` as its upgrade authority and funded parties.
pub fn setup() -> Env {
    let mut svm = LiteSVM::new();
    let bytes = include_bytes!(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/../deploy/inference_escrow.so"
    ));
    svm.add_program(program_id(), bytes).unwrap();

    let env = Env {
        svm,
        payer: Keypair::new(),
        admin: Keypair::new(),
        coordinator: Keypair::new(),
        client: Keypair::new(),
        owner1: Keypair::new(),
        owner2: Keypair::new(),
        attacker: Keypair::new(),
    };
    let mut env = env;

    // LiteSVM leaves the upgrade authority empty; set it to `admin` like `anchor deploy` would.
    // ProgramData metadata (bincode): variant u32 | slot u64 | Option tag u8 | authority [u8; 32].
    let pd = program_data_pda();
    let mut account = env.svm.get_account(&pd).unwrap();
    account.data[12] = 1;
    account.data[13..45].copy_from_slice(env.admin.pubkey().as_ref());
    env.svm.set_account(pd, account).unwrap();

    for key in [
        env.payer.pubkey(),
        env.admin.pubkey(),
        env.coordinator.pubkey(),
        env.client.pubkey(),
        env.owner1.pubkey(),
        env.owner2.pubkey(),
        env.attacker.pubkey(),
    ] {
        env.svm.airdrop(&key, 10_000_000_000).unwrap();
    }
    env
}

/// Sends `ixs` with `payer` as fee payer, signed by `payer` and `signers`.
pub fn send(env: &mut Env, ixs: &[Instruction], signers: &[&Keypair]) -> TransactionResult {
    // A fresh blockhash lets identical transactions (e.g. a repeated settle) be resent.
    env.svm.expire_blockhash();
    let mut all: Vec<&Keypair> = vec![&env.payer];
    all.extend_from_slice(signers);
    let msg =
        Message::new_with_blockhash(ixs, Some(&env.payer.pubkey()), &env.svm.latest_blockhash());
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &all).unwrap();
    env.svm.send_transaction(tx)
}

/// Asserts the transaction failed with the given custom error code (Anchor or program error).
pub fn assert_error(res: TransactionResult, code: u32) {
    let err = res.expect_err("transaction should fail").err;
    let debug = format!("{err:?}");
    assert!(
        debug.contains(&format!("Custom({code})")),
        "expected Custom({code}), got {debug}"
    );
}

pub fn balance(env: &Env, key: &Pubkey) -> u64 {
    env.svm.get_balance(key).unwrap_or(0)
}

pub fn request_rent(env: &Env) -> u64 {
    env.svm
        .minimum_balance_for_rent_exemption(8 + <Request as anchor_lang::Space>::INIT_SPACE)
}

pub fn read_worker(env: &Env, owner: &Pubkey) -> Worker {
    let account = env.svm.get_account(&worker_pda(owner)).unwrap();
    Worker::try_deserialize(&mut account.data.as_slice()).unwrap()
}

pub fn read_request(env: &Env, client: &Pubkey, nonce: u64) -> Option<Request> {
    let account = env.svm.get_account(&request_pda(client, nonce))?;
    if account.data.is_empty() {
        return None;
    }
    Some(Request::try_deserialize(&mut account.data.as_slice()).unwrap())
}

/// Overwrites a worker account, for states no instruction can reach yet (e.g. inactive).
pub fn edit_worker(env: &mut Env, owner: &Pubkey, edit: impl FnOnce(&mut Worker)) {
    let pda = worker_pda(owner);
    let mut account = env.svm.get_account(&pda).unwrap();
    let mut worker = Worker::try_deserialize(&mut account.data.as_slice()).unwrap();
    edit(&mut worker);
    let mut data = Vec::new();
    worker.try_serialize(&mut data).unwrap();
    account.data = data;
    env.svm.set_account(pda, account).unwrap();
}

// ---- Instruction builders ----

pub fn init_config_ix(
    authority: &Pubkey,
    coordinator: &Pubkey,
    fee_bps: u16,
    request_fee: u64,
) -> Instruction {
    Instruction::new_with_bytes(
        program_id(),
        &instruction::InitConfig {
            coordinator: *coordinator,
            fee_bps,
            request_fee,
        }
        .data(),
        accounts::InitConfig {
            authority: *authority,
            config: config_pda(),
            escrow_program: program_id(),
            program_data: program_data_pda(),
            system_program: system_program::ID,
            event_authority: event_authority(),
            program: program_id(),
        }
        .to_account_metas(None),
    )
}

pub fn register_worker_ix(owner: &Pubkey, offer: Offer) -> Instruction {
    Instruction::new_with_bytes(
        program_id(),
        &instruction::RegisterWorker {
            model: offer.model,
            tok_per_s: offer.tok_per_s,
            price_in: offer.price_in,
            price_out: offer.price_out,
        }
        .data(),
        accounts::RegisterWorker {
            owner: *owner,
            worker: worker_pda(owner),
            system_program: system_program::ID,
            event_authority: event_authority(),
            program: program_id(),
        }
        .to_account_metas(None),
    )
}

pub fn create_request_ix(
    client: &Pubkey,
    coordinator: &Pubkey,
    nonce: u64,
    terms: Terms,
) -> Instruction {
    Instruction::new_with_bytes(
        program_id(),
        &instruction::CreateRequest {
            nonce,
            model: terms.model,
            min_tps: terms.min_tps,
            max_price_in: terms.max_price_in,
            max_price_out: terms.max_price_out,
            budget: terms.budget,
            prompt_hash: PROMPT_HASH,
        }
        .data(),
        accounts::CreateRequest {
            client: *client,
            config: config_pda(),
            coordinator: *coordinator,
            request: request_pda(client, nonce),
            system_program: system_program::ID,
            event_authority: event_authority(),
            program: program_id(),
        }
        .to_account_metas(None),
    )
}

pub fn cancel_request_ix(signer: &Pubkey, request: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        program_id(),
        &instruction::CancelRequest {}.data(),
        accounts::CancelRequest {
            client: *signer,
            request,
            event_authority: event_authority(),
            program: program_id(),
        }
        .to_account_metas(None),
    )
}

pub fn assign_worker_ix(coordinator: &Pubkey, request: Pubkey, worker: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        program_id(),
        &instruction::AssignWorker {}.data(),
        accounts::AssignWorker {
            coordinator: *coordinator,
            config: config_pda(),
            request,
            worker,
            event_authority: event_authority(),
            program: program_id(),
        }
        .to_account_metas(None),
    )
}

pub struct SettleAccounts {
    pub coordinator: Pubkey,
    pub request: Pubkey,
    pub worker: Pubkey,
    pub owner: Pubkey,
    pub client: Pubkey,
}

pub fn settle_ix(
    a: &SettleAccounts,
    in_tokens: u64,
    out_tokens: u64,
    measured_tps: u32,
) -> Instruction {
    Instruction::new_with_bytes(
        program_id(),
        &instruction::Settle {
            in_tokens,
            out_tokens,
            measured_tps,
        }
        .data(),
        accounts::Settle {
            coordinator: a.coordinator,
            config: config_pda(),
            request: a.request,
            worker: a.worker,
            owner: a.owner,
            client: a.client,
            event_authority: event_authority(),
            program: program_id(),
        }
        .to_account_metas(None),
    )
}

// ---- Scenario steps ----

/// Config initialized, both workers registered. No request yet.
pub fn fixture() -> Env {
    let mut env = setup();
    let (admin, coordinator) = (env.admin.insecure_clone(), env.coordinator.pubkey());
    send(
        &mut env,
        &[init_config_ix(
            &admin.pubkey(),
            &coordinator,
            FEE_BPS,
            REQUEST_FEE,
        )],
        &[&admin],
    )
    .unwrap();
    let (o1, o2) = (env.owner1.insecure_clone(), env.owner2.insecure_clone());
    send(&mut env, &[register_worker_ix(&o1.pubkey(), W1)], &[&o1]).unwrap();
    send(&mut env, &[register_worker_ix(&o2.pubkey(), W2)], &[&o2]).unwrap();
    env
}

pub fn create_request(env: &mut Env, terms: Terms) -> TransactionResult {
    let client = env.client.insecure_clone();
    let ix = create_request_ix(&client.pubkey(), &env.coordinator.pubkey(), NONCE, terms);
    send(env, &[ix], &[&client])
}

pub fn default_request(env: &Env) -> Pubkey {
    request_pda(&env.client.pubkey(), NONCE)
}

pub fn assign(env: &mut Env, worker_owner: &Pubkey) -> TransactionResult {
    let coordinator = env.coordinator.insecure_clone();
    let ix = assign_worker_ix(
        &coordinator.pubkey(),
        default_request(env),
        worker_pda(worker_owner),
    );
    send(env, &[ix], &[&coordinator])
}

/// Honest settle accounts for the default request assigned to worker 1.
pub fn honest_settle(env: &Env) -> SettleAccounts {
    SettleAccounts {
        coordinator: env.coordinator.pubkey(),
        request: default_request(env),
        worker: worker_pda(&env.owner1.pubkey()),
        owner: env.owner1.pubkey(),
        client: env.client.pubkey(),
    }
}

pub fn settle(
    env: &mut Env,
    a: &SettleAccounts,
    in_tokens: u64,
    out_tokens: u64,
    tps: u32,
) -> TransactionResult {
    let coordinator = env.coordinator.insecure_clone();
    send(
        env,
        &[settle_ix(a, in_tokens, out_tokens, tps)],
        &[&coordinator],
    )
}
