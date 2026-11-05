//! End-to-end scenarios against a running stack: validator, coordinator, worker-1 (Qwen38, 40
//! tok/s, honest) and worker-2 (Llama4, 30 tok/s, `--faulty`). Exits non-zero on any mismatch.
//!
//! `cargo run -p cli --example e2e`

use std::{
    collections::BTreeSet,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, ensure, Context, Result};
use cli::stream;
use common::{
    chain::Chain,
    client::{instructions::CreateRequestInstructionArgs, types::Model},
    ix, keys, pricing,
    prompt::{count_tokens, new_salt, prompt_hash},
    protocol::{PromptRequest, StreamEvent},
    Address, Keypair, Signer,
};

const PROMPT: &str = "Explain Solana rent in one paragraph";
const MIN_TPS: u32 = 25;
const BUDGET: u64 = 1_000_000;
/// Every transaction here has a single signature.
const TX_FEE: u64 = 5_000;

struct Ctx {
    /// `$COORDINATOR_URL`, or the local coordinator.
    coordinator_url: String,
    chain: Chain,
    http: reqwest::Client,
    client: Keypair,
    coordinator: Address,
    request_fee: u64,
}

/// What the client saw for one request.
#[derive(Debug, Default)]
struct Outcome {
    worker: Option<Address>,
    out_tokens: u64,
    measured_tps: u32,
    due: Option<u64>,
    error: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let chain = Chain::from_env();
    let config = chain.config().await?.context("Config not initialized")?;
    let ctx = Ctx {
        coordinator_url: std::env::var("COORDINATOR_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8080".into()),
        chain,
        http: reqwest::Client::new(),
        client: keys::load("client-a")?,
        coordinator: config.coordinator,
        request_fee: config.request_fee,
    };

    let mut failed = 0;
    for (name, result) in [
        ("honest worker is paid for every token", honest(&ctx).await),
        (
            "faulty worker: slow, silent and crash are settled correctly",
            faulty(&ctx).await,
        ),
        (
            "request cancelled while queued is dropped",
            cancelled_while_queued(&ctx).await,
        ),
    ] {
        match result {
            Ok(()) => println!("PASS  {name}"),
            Err(e) => {
                failed += 1;
                println!("FAIL  {name}: {e:#}");
            }
        }
    }
    if failed > 0 {
        bail!("{failed} scenario(s) failed");
    }
    Ok(())
}

/// worker-1 streams the whole answer above the minimum throughput and gets the full price.
async fn honest(ctx: &Ctx) -> Result<()> {
    let before = ctx.chain.balance(&ctx.client.pubkey()).await?;
    let o = run(ctx, Model::Qwen38).await?;
    let worker = o.worker.context("never assigned")?;
    let due = o.due.context("never settled")?;
    ensure!(
        o.out_tokens > 0 && o.measured_tps >= MIN_TPS,
        "unexpected stream {o:?}"
    );
    ensure!(
        due == expected_due(ctx, &worker, &o).await?,
        "due {due} doesn't match the payout rule"
    );
    let spent = before - ctx.chain.balance(&ctx.client.pubkey()).await?;
    ensure!(
        spent == due + ctx.request_fee + TX_FEE,
        "client spent {spent}, expected due + request fee + tx fee"
    );
    Ok(())
}

/// worker-2 cycles slow → no start → crash per connection, so three requests hit all three
/// faults, in an order that depends on its earlier jobs.
async fn faulty(ctx: &Ctx) -> Result<()> {
    let mut seen = BTreeSet::new();
    for _ in 0..3 {
        let o = run(ctx, Model::Llama4).await?;
        let worker = o.worker.context("never assigned")?;
        let due = o.due.context("never settled")?;
        let fault = match (o.out_tokens, o.measured_tps) {
            (0, _) => "no-start",
            (_, tps) if tps < MIN_TPS => "slow",
            _ => "crash",
        };
        let expected = expected_due(ctx, &worker, &o).await?;
        ensure!(due == expected, "{fault}: due {due}, expected {expected}");
        match fault {
            "crash" => ensure!(due > 0 && o.out_tokens == 5, "crash: {o:?}"),
            _ => ensure!(due == 0, "{fault} should pay nothing: {o:?}"),
        }
        seen.insert(fault);
    }
    ensure!(seen.len() == 3, "expected all three faults, saw {seen:?}");
    Ok(())
}

/// While worker-1 streams a first request, a second one waits in the queue; cancelling it
/// on-chain makes the coordinator drop it instead of failing on `assign_worker`.
async fn cancelled_while_queued(ctx: &Ctx) -> Result<()> {
    let first = run(ctx, Model::Qwen38);
    let second = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (nonce, body) = create(ctx, Model::Qwen38).await?;
        let mut events = stream::submit(&ctx.http, &ctx.coordinator_url, &body).await?;
        ensure!(
            matches!(events.next().await, Some(Ok(StreamEvent::Queued))),
            "not queued"
        );
        let cancel = ix::cancel_request(&ctx.client.pubkey(), nonce);
        ctx.chain.send(&[cancel], &ctx.client, &[]).await?;
        let mut outcome = Outcome::default();
        while let Some(event) = events.next().await {
            record(&mut outcome, event?);
        }
        Ok(outcome)
    };
    let (first, second) = tokio::join!(first, second);
    ensure!(
        first?.due.is_some(),
        "the first request should still settle"
    );
    let second = second?;
    let error = second.error.as_deref().unwrap_or_default();
    ensure!(
        second.worker.is_none() && error.contains("no longer open"),
        "second request should be dropped, got {second:?}"
    );
    Ok(())
}

async fn create(ctx: &Ctx, model: Model) -> Result<(u64, PromptRequest)> {
    let salt = new_salt();
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as u64;
    let args = CreateRequestInstructionArgs {
        nonce,
        model,
        min_tps: MIN_TPS,
        max_price_in: 150,
        max_price_out: 500,
        budget: BUDGET,
        prompt_hash: prompt_hash(&salt, PROMPT),
    };
    let ix = ix::create_request(&ctx.client.pubkey(), &ctx.coordinator, args);
    ctx.chain.send(&[ix], &ctx.client, &[]).await?;
    let body = PromptRequest {
        client: ctx.client.pubkey().to_string(),
        nonce,
        prompt: PROMPT.into(),
        salt,
    };
    Ok((nonce, body))
}

async fn run(ctx: &Ctx, model: Model) -> Result<Outcome> {
    let (_, body) = create(ctx, model).await?;
    let mut events = stream::submit(&ctx.http, &ctx.coordinator_url, &body).await?;
    let mut outcome = Outcome::default();
    while let Some(event) = events.next().await {
        record(&mut outcome, event?);
    }
    Ok(outcome)
}

fn record(o: &mut Outcome, event: StreamEvent) {
    match event {
        StreamEvent::Assigned { worker, .. } => o.worker = worker.parse().ok(),
        StreamEvent::Settled {
            out_tokens,
            measured_tps,
            due,
            ..
        } => (o.out_tokens, o.measured_tps, o.due) = (out_tokens, measured_tps, Some(due)),
        StreamEvent::Error { reason } => o.error = Some(reason),
        StreamEvent::Queued | StreamEvent::Chunk { .. } => {}
    }
}

/// Payout rule applied to the worker's on-chain prices (frozen at assignment).
async fn expected_due(ctx: &Ctx, owner: &Address, o: &Outcome) -> Result<u64> {
    let w = ctx.chain.worker(owner).await?.context("worker account")?;
    pricing::compute_due(
        count_tokens(PROMPT),
        o.out_tokens,
        o.measured_tps,
        w.price_in,
        w.price_out,
        MIN_TPS,
        BUDGET,
    )
    .context("overflow")
}
