mod job;
mod matching;
mod requests;
mod state;
mod workers;

use std::{sync::Arc, time::Duration};

use anyhow::{bail, Context, Result};
use axum::{
    routing::{get, post},
    Router,
};
use clap::Parser;
use common::{
    chain::Chain,
    client::types::RequestStatus,
    ix::{self, Usage},
    keys, Keypair, Signer,
};
use tracing::{info, warn};

use crate::state::{App, Timeouts};

#[derive(Parser)]
#[command(about = "Coordinator: matches prompts with workers, relays streams, settles on-chain")]
struct Args {
    /// Address to listen on (0.0.0.0 inside a container).
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 8080)]
    port: u16,
    /// Keypair name in keys/; must be `Config::coordinator`.
    #[arg(long, default_value = "coordinator")]
    key: String,
    /// Seconds to wait for a worker's first token.
    #[arg(long, default_value_t = 5)]
    first_chunk_timeout: u64,
    /// Seconds to wait between two tokens.
    #[arg(long, default_value_t = 3)]
    chunk_timeout: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let args = Args::parse();
    let key = keys::load(&args.key)?;
    let chain = Chain::from_env();

    let config = chain
        .config()
        .await?
        .context("Config not initialized (run init_config first)")?;
    if config.coordinator != key.pubkey() {
        bail!(
            "Config.coordinator is {}, not this key ({})",
            config.coordinator,
            key.pubkey()
        );
    }
    recover(&chain, &key).await?;

    let timeouts = Timeouts {
        first_chunk: Duration::from_secs(args.first_chunk_timeout),
        chunk: Duration::from_secs(args.chunk_timeout),
    };
    let app = Arc::new(App::new(chain, key, config.fee_bps, timeouts));
    let router = Router::new()
        .route("/workers/ws", get(workers::handler))
        .route("/requests", post(requests::submit))
        .with_state(app);

    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port)).await?;
    info!("coordinator listening on {}", listener.local_addr()?);
    axum::serve(listener, router).await?;
    Ok(())
}

/// Nothing is persisted: requests left `Assigned` by a previous run have lost their stream, so
/// they are settled with zero usage, which refunds their clients in full.
async fn recover(chain: &Chain, key: &Keypair) -> Result<()> {
    for (address, request) in chain.requests().await? {
        if request.status != RequestStatus::Assigned {
            continue;
        }
        let Some(worker) = chain.worker_at(&request.worker).await? else {
            warn!(%address, "assigned worker account missing; cannot settle");
            continue;
        };
        let usage = Usage {
            in_tokens: 0,
            out_tokens: 0,
            measured_tps: 0,
        };
        let ix = ix::settle(
            &key.pubkey(),
            &request.client,
            request.nonce,
            &worker.owner,
            usage,
        );
        match chain.send(&[ix], key, &[]).await {
            Ok(signature) => info!(%address, %signature, "recovered: refunded assigned request"),
            Err(e) => warn!(%address, "recovery settle failed: {e:#}"),
        }
    }
    Ok(())
}
