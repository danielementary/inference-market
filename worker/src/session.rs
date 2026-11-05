use std::time::Duration;

use anyhow::{bail, Context, Result};
use common::{
    client::types::Model,
    protocol::{sign_challenge, CoordinatorMsg, WorkerMsg},
    Keypair, Signer,
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::info;

use crate::{
    fault::{Fault, CRASH_AFTER},
    responses,
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// What the worker serves; mirrors its on-chain offer.
#[derive(Clone, Copy)]
pub struct Offer {
    pub model: Model,
    pub tok_per_s: u32,
}

/// One connection to the coordinator: handshake, then jobs until the connection drops.
/// Returns an error on disconnect (including simulated crashes) so the caller reconnects.
/// A `faulty` worker misbehaves on every job, cycling through `Fault`.
pub async fn run(url: &str, owner: &Keypair, offer: Offer, faulty: bool) -> Result<()> {
    let (mut ws, _) = connect_async(url)
        .await
        .with_context(|| format!("connecting to {url}"))?;

    let CoordinatorMsg::Challenge { nonce } = recv(&mut ws).await? else {
        bail!("expected a challenge");
    };
    let signature = sign_challenge(owner, &nonce);
    send(
        &mut ws,
        &WorkerMsg::Auth {
            owner: owner.pubkey().to_string(),
            signature,
        },
    )
    .await?;
    if recv(&mut ws).await? != CoordinatorMsg::Ready {
        bail!("coordinator refused authentication");
    }
    info!("connected to {url}");

    for job in 0.. {
        let CoordinatorMsg::Job {
            request,
            prompt,
            max_out_tokens,
        } = recv(&mut ws).await?
        else {
            bail!("expected a job");
        };
        let fault = faulty.then(|| Fault::for_job(job));
        info!(%request, ?fault, prompt_words = prompt.split_whitespace().count(), "job");

        let (rate, limit) = match fault {
            None => (offer.tok_per_s, max_out_tokens),
            Some(Fault::Slow) => ((offer.tok_per_s / 2).max(1), max_out_tokens),
            Some(Fault::NoStart) => continue,
            Some(Fault::Crash) => (offer.tok_per_s, CRASH_AFTER.min(max_out_tokens)),
        };
        stream(&mut ws, &request, offer.model, rate, limit).await?;

        if fault == Some(Fault::Crash) {
            // Drop the socket without a close frame or `Done`, like a crashed process.
            bail!("simulated crash");
        }
        send(&mut ws, &WorkerMsg::Done { request }).await?;
    }
    unreachable!()
}

/// Sends up to `limit` tokens, one every `1 / rate` seconds.
async fn stream(ws: &mut Ws, request: &str, model: Model, rate: u32, limit: u64) -> Result<()> {
    let interval = Duration::from_secs_f64(1.0 / rate as f64);
    for (seq, token) in responses::tokens(model)
        .into_iter()
        .take(limit as usize)
        .enumerate()
    {
        tokio::time::sleep(interval).await;
        let chunk = WorkerMsg::Chunk {
            request: request.into(),
            seq: seq as u64,
            token: token.into(),
        };
        send(ws, &chunk).await?;
    }
    Ok(())
}

async fn send(ws: &mut Ws, msg: &WorkerMsg) -> Result<()> {
    ws.send(Message::text(serde_json::to_string(msg)?)).await?;
    Ok(())
}

async fn recv(ws: &mut Ws) -> Result<CoordinatorMsg> {
    while let Some(frame) = ws.next().await {
        match frame? {
            Message::Text(text) => return Ok(serde_json::from_str(&text)?),
            Message::Close(_) => break,
            _ => {} // pings are answered by tungstenite
        }
    }
    bail!("connection closed by the coordinator")
}
