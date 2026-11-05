//! `GET /workers/ws`: worker connections.

use std::{sync::Arc, time::Duration};

use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::Response,
};
use common::{
    protocol::{verify_challenge, CoordinatorMsg, WorkerMsg},
    Address,
};
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use tokio::{sync::mpsc, time::timeout};
use tracing::{info, warn};

use crate::state::App;

pub async fn handler(ws: WebSocketUpgrade, State(app): State<Arc<App>>) -> Response {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = session(app, socket).await {
            warn!("worker session: {e:#}");
        }
    })
}

/// Handshake (signed challenge + on-chain offer check), then forwards the worker's messages to
/// its current job until it disconnects.
async fn session(app: Arc<App>, socket: WebSocket) -> Result<()> {
    let (mut sink, mut stream) = socket.split();

    let nonce: [u8; 32] = rand::random();
    send(&mut sink, &CoordinatorMsg::Challenge { nonce }).await?;
    let Some(WorkerMsg::Auth { owner, signature }) =
        timeout(Duration::from_secs(5), recv(&mut stream))
            .await
            .context("no auth in time")?
    else {
        bail!("expected auth");
    };
    let owner: Address = owner.parse().context("invalid owner")?;
    ensure!(
        verify_challenge(&owner, &nonce, &signature),
        "invalid signature from {owner}"
    );
    let offer = app
        .chain
        .worker(&owner)
        .await?
        .with_context(|| format!("{owner} has no Worker account"))?;
    ensure!(offer.active, "worker {owner} is inactive");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let conn = app
        .add_worker(owner, offer.clone(), tx)
        .with_context(|| format!("{owner} is already connected"))?;
    send(&mut sink, &CoordinatorMsg::Ready).await?;
    info!(%owner, model = ?offer.model, tps = offer.tok_per_s, "worker online");

    // Jobs queued for this worker are written only after `Ready`.
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if send(&mut sink, &msg).await.is_err() {
                break;
            }
        }
    });
    app.dispatch();

    while let Some(msg) = recv(&mut stream).await {
        app.route(&owner, conn, msg);
    }
    app.remove_worker(&owner, conn);
    writer.abort();
    info!(%owner, "worker offline");
    Ok(())
}

async fn send(sink: &mut SplitSink<WebSocket, Message>, msg: &CoordinatorMsg) -> Result<()> {
    sink.send(Message::Text(serde_json::to_string(msg)?.into()))
        .await?;
    Ok(())
}

/// Next parsed message; `None` once the connection is closed. Malformed messages are skipped.
async fn recv(stream: &mut SplitStream<WebSocket>) -> Option<WorkerMsg> {
    while let Some(Ok(frame)) = stream.next().await {
        match frame {
            Message::Text(text) => match serde_json::from_str(&text) {
                Ok(msg) => return Some(msg),
                Err(e) => warn!("malformed worker message: {e}"),
            },
            Message::Close(_) => return None,
            _ => {}
        }
    }
    None
}
