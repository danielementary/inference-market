mod common;

use {
    anchor_lang::{error::ErrorCode as AnchorError, AnchorDeserialize, Discriminator},
    common::*,
    inference_escrow::{error::EscrowError, events::RequestSettled, RequestStatus},
    solana_signer::Signer,
};

fn escrow(e: EscrowError) -> u32 {
    u32::from(e)
}

fn anchor(e: AnchorError) -> u32 {
    u32::from(e)
}

/// Default request created and assigned to worker 1.
fn assigned() -> Env {
    let mut env = fixture();
    create_request(&mut env, TERMS).unwrap();
    let owner1 = env.owner1.pubkey();
    assign(&mut env, &owner1).unwrap();
    env
}

// ---- Happy paths ----

#[test]
fn full_flow_pays_worker_coordinator_and_refunds_client() {
    let mut env = fixture();
    let (client, owner, coordinator) = (
        env.client.pubkey(),
        env.owner1.pubkey(),
        env.coordinator.pubkey(),
    );
    let before = [
        balance(&env, &client),
        balance(&env, &owner),
        balance(&env, &coordinator),
    ];

    create_request(&mut env, TERMS).unwrap();
    assign(&mut env, &owner).unwrap();
    let request = read_request(&env, &client, NONCE).unwrap();
    assert_eq!(request.status, RequestStatus::Assigned);
    assert_eq!(
        (request.agreed_price_in, request.agreed_price_out),
        (W1.price_in, W1.price_out)
    );

    let accounts = honest_settle(&env);
    settle(&mut env, &accounts, 200, 1_500, 40).unwrap();

    // 200 × 100 + 1 500 × 400 = 620 000, fee 2 % = 12 400.
    let (due, fee) = (620_000, 12_400);
    assert_eq!(balance(&env, &client), before[0] - due - REQUEST_FEE);
    assert_eq!(balance(&env, &owner), before[1] + due - fee);
    assert_eq!(balance(&env, &coordinator), before[2] + fee + REQUEST_FEE);
    assert!(
        read_request(&env, &client, NONCE).is_none(),
        "request account must be closed"
    );

    // Conservation: lamports only moved between the three parties.
    let after = balance(&env, &client) + balance(&env, &owner) + balance(&env, &coordinator);
    assert_eq!(after, before.iter().sum::<u64>());
}

#[test]
fn cancel_refunds_budget_and_rent_but_not_request_fee() {
    let mut env = fixture();
    let client = env.client.insecure_clone();
    let before = balance(&env, &client.pubkey());

    create_request(&mut env, TERMS).unwrap();
    assert_eq!(
        balance(&env, &client.pubkey()),
        before - BUDGET - REQUEST_FEE - request_rent(&env)
    );

    {
        let ix = cancel_request_ix(&client.pubkey(), default_request(&env));
        send(&mut env, &[ix], &[&client])
    }
    .unwrap();
    assert_eq!(balance(&env, &client.pubkey()), before - REQUEST_FEE);
    assert!(read_request(&env, &client.pubkey(), NONCE).is_none());
}

#[test]
fn nonce_can_be_reused_after_close() {
    let mut env = fixture();
    let client = env.client.insecure_clone();
    create_request(&mut env, TERMS).unwrap();
    {
        let ix = cancel_request_ix(&client.pubkey(), default_request(&env));
        send(&mut env, &[ix], &[&client])
    }
    .unwrap();
    create_request(&mut env, TERMS).unwrap();
}

#[test]
fn nonce_cannot_be_reused_while_open() {
    let mut env = fixture();
    let client = env.client.pubkey();
    create_request(&mut env, TERMS).unwrap();
    let before = balance(&env, &client);

    // `init` fails in the system program: account already in use (SystemError 0).
    let res = create_request(
        &mut env,
        Terms {
            budget: 5 * BUDGET,
            ..TERMS
        },
    );
    assert_error(res, 0);

    // The open request and the client's funds are untouched.
    assert_eq!(read_request(&env, &client, NONCE).unwrap().budget, BUDGET);
    assert_eq!(balance(&env, &client), before);
}

#[test]
fn nonce_cannot_be_reused_while_assigned() {
    let mut env = assigned();
    let res = create_request(&mut env, TERMS);
    assert_error(res, 0);
    let request = read_request(&env, &env.client.pubkey(), NONCE).unwrap();
    assert_eq!(request.status, RequestStatus::Assigned);
}

// ---- Funds: authorization and account substitution ----

#[test]
fn init_config_rejects_non_upgrade_authority() {
    let mut env = setup();
    let attacker = env.attacker.insecure_clone();
    let ix = init_config_ix(&attacker.pubkey(), &attacker.pubkey(), FEE_BPS, REQUEST_FEE);
    assert_error(
        send(&mut env, &[ix], &[&attacker]),
        anchor(AnchorError::ConstraintRaw),
    );
}

#[test]
fn init_config_only_once() {
    let mut env = fixture();
    let admin = env.admin.insecure_clone();
    let ix = init_config_ix(&admin.pubkey(), &admin.pubkey(), FEE_BPS, REQUEST_FEE);
    // The system program refuses to create an account that already exists.
    assert!(send(&mut env, &[ix], &[&admin]).is_err());
}

#[test]
fn assign_rejects_non_coordinator() {
    let mut env = fixture();
    create_request(&mut env, TERMS).unwrap();
    let attacker = env.attacker.insecure_clone();
    let ix = assign_worker_ix(
        &attacker.pubkey(),
        default_request(&env),
        worker_pda(&env.owner1.pubkey()),
    );
    assert_error(
        send(&mut env, &[ix], &[&attacker]),
        anchor(AnchorError::ConstraintHasOne),
    );
}

#[test]
fn settle_rejects_non_coordinator() {
    let mut env = assigned();
    let attacker = env.attacker.insecure_clone();
    let accounts = SettleAccounts {
        coordinator: attacker.pubkey(),
        ..honest_settle(&env)
    };
    let ix = settle_ix(&accounts, 200, 1_500, 40);
    assert_error(
        send(&mut env, &[ix], &[&attacker]),
        anchor(AnchorError::ConstraintHasOne),
    );
}

#[test]
fn settle_rejects_other_worker() {
    let mut env = assigned();
    let owner2 = env.owner2.pubkey();
    let accounts = SettleAccounts {
        worker: worker_pda(&owner2),
        owner: owner2,
        ..honest_settle(&env)
    };
    assert_error(
        settle(&mut env, &accounts, 200, 1_500, 40),
        anchor(AnchorError::ConstraintHasOne),
    );
}

#[test]
fn settle_rejects_wrong_owner() {
    let mut env = assigned();
    let accounts = SettleAccounts {
        owner: env.attacker.pubkey(),
        ..honest_settle(&env)
    };
    assert_error(
        settle(&mut env, &accounts, 200, 1_500, 40),
        anchor(AnchorError::ConstraintHasOne),
    );
}

#[test]
fn settle_rejects_wrong_client() {
    let mut env = assigned();
    let accounts = SettleAccounts {
        client: env.attacker.pubkey(),
        ..honest_settle(&env)
    };
    assert_error(
        settle(&mut env, &accounts, 200, 1_500, 40),
        anchor(AnchorError::ConstraintHasOne),
    );
}

#[test]
fn settle_twice_fails() {
    let mut env = assigned();
    let accounts = honest_settle(&env);
    settle(&mut env, &accounts, 200, 1_500, 40).unwrap();
    assert_error(
        settle(&mut env, &accounts, 200, 1_500, 40),
        anchor(AnchorError::AccountNotInitialized),
    );
}

#[test]
fn create_rejects_wrong_coordinator_account() {
    let mut env = fixture();
    let client = env.client.insecure_clone();
    let ix = create_request_ix(&client.pubkey(), &env.attacker.pubkey(), NONCE, TERMS);
    assert_error(
        send(&mut env, &[ix], &[&client]),
        anchor(AnchorError::ConstraintAddress),
    );
}

#[test]
fn cancel_by_other_signer_fails() {
    let mut env = fixture();
    create_request(&mut env, TERMS).unwrap();
    let attacker = env.attacker.insecure_clone();
    let ix = cancel_request_ix(&attacker.pubkey(), default_request(&env));
    assert_error(
        send(&mut env, &[ix], &[&attacker]),
        anchor(AnchorError::ConstraintHasOne),
    );
}

#[test]
fn cancel_after_assign_fails() {
    let mut env = assigned();
    let client = env.client.insecure_clone();
    let ix = cancel_request_ix(&client.pubkey(), default_request(&env));
    assert_error(
        send(&mut env, &[ix], &[&client]),
        escrow(EscrowError::InvalidStatus),
    );
}

// ---- Accepted terms ----

#[test]
fn assign_rejects_model_mismatch() {
    let mut env = fixture();
    create_request(&mut env, TERMS).unwrap();
    let owner2 = env.owner2.pubkey();
    assert_error(
        assign(&mut env, &owner2),
        escrow(EscrowError::ModelMismatch),
    );
}

#[test]
fn assign_rejects_low_throughput() {
    let mut env = fixture();
    create_request(
        &mut env,
        Terms {
            min_tps: W1.tok_per_s + 1,
            ..TERMS
        },
    )
    .unwrap();
    let owner1 = env.owner1.pubkey();
    assert_error(
        assign(&mut env, &owner1),
        escrow(EscrowError::ThroughputTooLow),
    );
}

#[test]
fn assign_rejects_price_above_max() {
    for terms in [
        Terms {
            max_price_in: W1.price_in - 1,
            ..TERMS
        },
        Terms {
            max_price_out: W1.price_out - 1,
            ..TERMS
        },
    ] {
        let mut env = fixture();
        create_request(&mut env, terms).unwrap();
        let owner1 = env.owner1.pubkey();
        assert_error(
            assign(&mut env, &owner1),
            escrow(EscrowError::PriceAboveMax),
        );
    }
}

#[test]
fn assign_rejects_inactive_worker() {
    let mut env = fixture();
    create_request(&mut env, TERMS).unwrap();
    let owner1 = env.owner1.pubkey();
    edit_worker(&mut env, &owner1, |w| w.active = false);
    assert_error(
        assign(&mut env, &owner1),
        escrow(EscrowError::WorkerInactive),
    );
}

#[test]
fn assign_twice_fails() {
    let mut env = assigned();
    let owner1 = env.owner1.pubkey();
    assert_error(
        assign(&mut env, &owner1),
        escrow(EscrowError::InvalidStatus),
    );
}

#[test]
fn settle_uses_frozen_prices() {
    let mut env = assigned();
    let owner1 = env.owner1.pubkey();
    // The worker raises its prices after assignment; the request keeps the agreed ones.
    edit_worker(&mut env, &owner1, |w| {
        w.price_in *= 10;
        w.price_out *= 10;
    });
    let before = balance(&env, &owner1);
    let accounts = honest_settle(&env);
    settle(&mut env, &accounts, 200, 1_500, 40).unwrap();
    assert_eq!(balance(&env, &owner1), before + 620_000 - 12_400);
}

#[test]
fn settle_below_min_tps_pays_nothing() {
    let mut env = assigned();
    let (client, owner1) = (env.client.pubkey(), env.owner1.pubkey());
    let (client_before, owner_before) = (balance(&env, &client), balance(&env, &owner1));
    let accounts = honest_settle(&env);
    settle(&mut env, &accounts, 200, 1_500, TERMS.min_tps - 1).unwrap();
    assert_eq!(balance(&env, &owner1), owner_before);
    assert_eq!(
        balance(&env, &client),
        client_before + BUDGET + request_rent(&env)
    );
}

#[test]
fn settle_caps_due_at_budget() {
    let mut env = assigned();
    let (client, owner1) = (env.client.pubkey(), env.owner1.pubkey());
    let (client_before, owner_before) = (balance(&env, &client), balance(&env, &owner1));
    let accounts = honest_settle(&env);
    // 10 000 output tokens × 400 = 4 000 000 > budget.
    settle(&mut env, &accounts, 0, 10_000, 40).unwrap();
    let fee = BUDGET * FEE_BPS as u64 / 10_000;
    assert_eq!(balance(&env, &owner1), owner_before + BUDGET - fee);
    assert_eq!(balance(&env, &client), client_before + request_rent(&env));
}

#[test]
fn settle_zero_tokens_is_full_refund() {
    let mut env = assigned();
    let client = env.client.pubkey();
    let before = balance(&env, &client);
    let accounts = honest_settle(&env);
    settle(&mut env, &accounts, 0, 0, 0).unwrap();
    assert_eq!(balance(&env, &client), before + BUDGET + request_rent(&env));
}

#[test]
fn settle_overflow_is_rejected() {
    let mut env = fixture();
    let owner1 = env.owner1.pubkey();
    edit_worker(&mut env, &owner1, |w| {
        w.price_in = u64::MAX;
        w.price_out = u64::MAX;
    });
    create_request(
        &mut env,
        Terms {
            max_price_in: u64::MAX,
            max_price_out: u64::MAX,
            ..TERMS
        },
    )
    .unwrap();
    assign(&mut env, &owner1).unwrap();
    let accounts = honest_settle(&env);
    assert_error(
        settle(&mut env, &accounts, u64::MAX, u64::MAX, 40),
        escrow(EscrowError::MathOverflow),
    );
}

// ---- Parameters ----

#[test]
fn init_config_rejects_fee_above_max() {
    let mut env = setup();
    let admin = env.admin.insecure_clone();
    let ix = init_config_ix(
        &admin.pubkey(),
        &env.coordinator.pubkey(),
        1_001,
        REQUEST_FEE,
    );
    assert_error(
        send(&mut env, &[ix], &[&admin]),
        escrow(EscrowError::InvalidParams),
    );
}

#[test]
fn create_rejects_zero_budget() {
    let mut env = fixture();
    assert_error(
        create_request(&mut env, Terms { budget: 0, ..TERMS }),
        escrow(EscrowError::InvalidParams),
    );
}

#[test]
fn register_rejects_zero_throughput() {
    let mut env = fixture();
    let attacker = env.attacker.insecure_clone();
    let ix = register_worker_ix(&attacker.pubkey(), Offer { tok_per_s: 0, ..W1 });
    assert_error(
        send(&mut env, &[ix], &[&attacker]),
        escrow(EscrowError::InvalidParams),
    );
}

// ---- Events and costs ----

#[test]
fn settle_emits_event_via_cpi() {
    let mut env = assigned();
    let accounts = honest_settle(&env);
    let meta = settle(&mut env, &accounts, 200, 1_500, 40).unwrap();

    let event = meta
        .inner_instructions
        .iter()
        .flatten()
        .map(|ix| ix.instruction.data.as_slice())
        .filter_map(|data| data.strip_prefix(anchor_lang::event::EVENT_IX_TAG_LE))
        .filter_map(|data| data.strip_prefix(RequestSettled::DISCRIMINATOR))
        .map(|mut data| RequestSettled::deserialize(&mut data).unwrap())
        .next()
        .expect("RequestSettled event in inner instructions");

    assert_eq!(event.request, default_request(&env));
    assert_eq!(
        (event.in_tokens, event.out_tokens, event.measured_tps),
        (200, 1_500, 40)
    );
    assert_eq!(
        (event.due, event.fee, event.refund),
        (620_000, 12_400, BUDGET - 620_000)
    );
}

#[test]
fn measure_compute_units() {
    let mut env = setup();
    let admin = env.admin.insecure_clone();
    let coordinator = env.coordinator.pubkey();
    let owner1 = env.owner1.insecure_clone();
    let client = env.client.insecure_clone();

    let mut rows = Vec::new();
    let meta = send(
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
    rows.push(("init_config", meta.compute_units_consumed));
    let meta = send(
        &mut env,
        &[register_worker_ix(&owner1.pubkey(), W1)],
        &[&owner1],
    )
    .unwrap();
    rows.push(("register_worker", meta.compute_units_consumed));
    rows.push((
        "create_request",
        create_request(&mut env, TERMS)
            .unwrap()
            .compute_units_consumed,
    ));
    rows.push((
        "assign_worker",
        assign(&mut env, &owner1.pubkey())
            .unwrap()
            .compute_units_consumed,
    ));
    let accounts = honest_settle(&env);
    rows.push((
        "settle",
        settle(&mut env, &accounts, 200, 1_500, 40)
            .unwrap()
            .compute_units_consumed,
    ));

    create_request(&mut env, TERMS).unwrap();
    let meta = {
        let ix = cancel_request_ix(&client.pubkey(), default_request(&env));
        send(&mut env, &[ix], &[&client])
    }
    .unwrap();
    rows.push(("cancel_request", meta.compute_units_consumed));

    println!("\n| Instruction | CU |\n|---|---|");
    for (name, cu) in rows {
        println!("| `{name}` | {cu} |");
    }
}
