//! One builder per program instruction. Fills in what the generated client leaves to the
//! caller: PDAs, the `emit_cpi` event authority, the program account and `ProgramData`.

use inference_escrow_client::{
    accounts::{Config, Request, Worker},
    instructions::*,
    types::Model,
    ID,
};
use solana_address::{address, Address};
use solana_transaction::Instruction;

const SYSTEM_PROGRAM: Address = address!("11111111111111111111111111111111");
const BPF_LOADER_UPGRADEABLE: Address = address!("BPFLoaderUpgradeab1e11111111111111111111111");

pub fn config_pda() -> Address {
    Config::find_pda().0
}

pub fn worker_pda(owner: &Address) -> Address {
    Worker::find_pda(owner).0
}

pub fn request_pda(client: &Address, nonce: u64) -> Address {
    Request::find_pda(client, nonce).0
}

/// PDA that signs the self-CPI carrying each `emit_cpi!` event.
pub fn event_authority() -> Address {
    Address::find_program_address(&[b"__event_authority"], &ID).0
}

fn program_data() -> Address {
    Address::find_program_address(&[ID.as_ref()], &BPF_LOADER_UPGRADEABLE).0
}

/// Must be signed by the program's upgrade authority.
pub fn init_config(
    authority: &Address,
    coordinator: Address,
    fee_bps: u16,
    request_fee: u64,
) -> Instruction {
    InitConfig {
        authority: *authority,
        config: config_pda(),
        escrow_program: ID,
        program_data: program_data(),
        system_program: SYSTEM_PROGRAM,
        event_authority: event_authority(),
        program: ID,
    }
    .instruction(InitConfigInstructionArgs {
        coordinator,
        fee_bps,
        request_fee,
    })
}

pub fn register_worker(
    owner: &Address,
    model: Model,
    tok_per_s: u32,
    price_in: u64,
    price_out: u64,
) -> Instruction {
    RegisterWorker {
        owner: *owner,
        worker: worker_pda(owner),
        system_program: SYSTEM_PROGRAM,
        event_authority: event_authority(),
        program: ID,
    }
    .instruction(RegisterWorkerInstructionArgs {
        model,
        tok_per_s,
        price_in,
        price_out,
    })
}

/// `coordinator` must be `Config::coordinator`; it receives the request fee.
pub fn create_request(
    client: &Address,
    coordinator: &Address,
    args: CreateRequestInstructionArgs,
) -> Instruction {
    CreateRequest {
        client: *client,
        config: config_pda(),
        coordinator: *coordinator,
        request: request_pda(client, args.nonce),
        system_program: SYSTEM_PROGRAM,
        event_authority: event_authority(),
        program: ID,
    }
    .instruction(args)
}

pub fn cancel_request(client: &Address, nonce: u64) -> Instruction {
    CancelRequest {
        client: *client,
        request: request_pda(client, nonce),
        event_authority: event_authority(),
        program: ID,
    }
    .instruction()
}

pub fn assign_worker(
    coordinator: &Address,
    request: Address,
    worker_owner: &Address,
) -> Instruction {
    AssignWorker {
        coordinator: *coordinator,
        config: config_pda(),
        request,
        worker: worker_pda(worker_owner),
        event_authority: event_authority(),
        program: ID,
    }
    .instruction()
}

pub struct Usage {
    pub in_tokens: u64,
    pub out_tokens: u64,
    pub measured_tps: u32,
}

pub fn settle(
    coordinator: &Address,
    client: &Address,
    nonce: u64,
    worker_owner: &Address,
    usage: Usage,
) -> Instruction {
    Settle {
        coordinator: *coordinator,
        config: config_pda(),
        request: request_pda(client, nonce),
        worker: worker_pda(worker_owner),
        owner: *worker_owner,
        client: *client,
        event_authority: event_authority(),
        program: ID,
    }
    .instruction(SettleInstructionArgs {
        in_tokens: usage.in_tokens,
        out_tokens: usage.out_tokens,
        measured_tps: usage.measured_tps,
    })
}
