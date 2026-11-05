//! One job: assign on-chain → relay and meter the stream → settle on-chain.

use std::{sync::Arc, time::Instant};

use anyhow::{bail, Context, Result};
use common::{
    client::types::RequestStatus,
    ix::{self, Usage},
    pricing,
    protocol::{CoordinatorMsg, StreamEvent, WorkerMsg},
    Address, Signer,
};
use tokio::time::timeout;
use tracing::{info, warn};

use crate::{
    matching,
    state::{App, Job, Pending},
};

pub async fn run(app: Arc<App>, job: Job) {
    let (request, owner, conn) = (job.pending.request, job.owner, job.conn);
    let events = job.pending.events.clone();
    if let Err(e) = drive(&app, job).await {
        warn!(%request, "job failed: {e:#}");
        let _ = events
            .send(StreamEvent::Error {
                reason: format!("{e:#}"),
            })
            .await;
    }
    app.finish(&request, &owner, conn);
    app.dispatch();
}

async fn drive(app: &App, job: Job) -> Result<()> {
    let Job {
        pending: p,
        owner,
        offer,
        worker_tx,
        mut messages,
        ..
    } = job;

    // The client may have cancelled while the prompt was queued.
    match app.chain.request(&p.client, p.nonce).await? {
        Some(r) if r.status == RequestStatus::Open => {}
        _ => bail!("request is no longer open (cancelled?)"),
    }

    let assign = ix::assign_worker(&app.key.pubkey(), p.request, &owner);
    let signature = app
        .chain
        .send(&[assign], &app.key, &[])
        .await
        .context("assign_worker")?;
    info!(request = %p.request, worker = %owner, "assigned");
    let _ = p
        .events
        .send(StreamEvent::Assigned {
            worker: owner.to_string(),
            signature,
        })
        .await;

    // Prices are frozen at assignment; the worker can't change them (no update_worker yet).
    let max_out =
        matching::max_out_tokens(p.terms.budget, p.in_tokens, offer.price_in, offer.price_out);
    let started = Instant::now();
    let job = CoordinatorMsg::Job {
        request: p.request.to_string(),
        prompt: p.prompt.clone(),
        max_out_tokens: max_out,
    };
    let (out_tokens, last) = match worker_tx.send(job) {
        Ok(()) => relay(app, &p, &mut messages, max_out).await,
        Err(_) => (0, None), // worker disconnected meanwhile
    };
    let measured_tps = last.map_or(0, |last| matching::measured_tps(out_tokens, last - started));
    info!(request = %p.request, out_tokens, measured_tps, "stream over");

    let usage = Usage {
        in_tokens: p.in_tokens,
        out_tokens,
        measured_tps,
    };
    let signature = settle(app, &p, &owner, usage).await?;
    let due = pricing::compute_due(
        p.in_tokens,
        out_tokens,
        measured_tps,
        offer.price_in,
        offer.price_out,
        p.terms.min_tps,
        p.terms.budget,
    )
    .unwrap_or(0);
    let fee = pricing::compute_fee(due, app.fee_bps);
    info!(request = %p.request, due, fee, "settled");
    let settled = StreamEvent::Settled {
        signature,
        in_tokens: p.in_tokens,
        out_tokens,
        measured_tps,
        due,
        fee,
        refund: p.terms.budget - due,
    };
    let _ = p.events.send(settled).await;
    Ok(())
}

/// Relays tokens to the client and counts them itself, until `Done`, a disconnect, a timeout
/// or `max_out` tokens. Returns the count and the time of the last token.
async fn relay(
    app: &App,
    p: &Pending,
    messages: &mut tokio::sync::mpsc::UnboundedReceiver<WorkerMsg>,
    max_out: u64,
) -> (u64, Option<Instant>) {
    let request = p.request.to_string();
    let (mut out, mut last) = (0u64, None);
    while out < max_out {
        let wait = if out == 0 {
            app.timeouts.first_chunk
        } else {
            app.timeouts.chunk
        };
        match timeout(wait, messages.recv()).await {
            Err(_) => {
                info!(%request, out, "worker timed out");
                break;
            }
            Ok(None) => {
                info!(%request, out, "worker disconnected");
                break;
            }
            // The worker's `seq` isn't trusted: the coordinator counts what it receives.
            Ok(Some(WorkerMsg::Chunk {
                request: r, token, ..
            })) if r == request => {
                out += 1;
                last = Some(Instant::now());
                // A client that went away doesn't stop the job: produced tokens are still paid.
                let _ = p.events.send(StreamEvent::Chunk { token }).await;
            }
            Ok(Some(WorkerMsg::Done { request: r } | WorkerMsg::Failed { request: r, .. }))
                if r == request =>
            {
                break
            }
            Ok(Some(_)) => {} // late message from an earlier job, or protocol noise
        }
    }
    (out, last)
}

/// Sends `settle`, retrying a couple of times; an already closed request counts as settled.
async fn settle(app: &App, p: &Pending, owner: &Address, usage: Usage) -> Result<String> {
    let ixs = [ix::settle(
        &app.key.pubkey(),
        &p.client,
        p.nonce,
        owner,
        usage,
    )];
    for attempt in 1..=3 {
        match app.chain.send(&ixs, &app.key, &[]).await {
            Ok(signature) => return Ok(signature),
            Err(e) => {
                warn!(request = %p.request, attempt, "settle failed: {e:#}");
                if app.chain.request(&p.client, p.nonce).await?.is_none() {
                    return Ok("already settled".into());
                }
            }
        }
    }
    bail!("settle failed after 3 attempts; the request stays assigned until the next restart")
}
