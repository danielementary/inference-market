use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use clap::Parser;
use common::chain::Chain;
use indexer::{api, index::Index, sync::sync};
use tracing::{info, warn};

#[derive(Parser)]
#[command(about = "Rebuilds the escrow program's history from its transactions and serves it")]
struct Args {
    /// Address to listen on (0.0.0.0 inside a container).
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 8090)]
    port: u16,
    /// Milliseconds between two syncs.
    #[arg(long, default_value_t = 1_000)]
    interval_ms: u64,
    /// Also log the events replayed at startup, not only the new ones.
    #[arg(long)]
    verbose_replay: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let args = Args::parse();
    let chain = Chain::from_env();

    // Nothing is persisted: every start replays the program's whole history.
    let mut index = Index::default();
    index.log_events = args.verbose_replay;
    let index = Arc::new(Mutex::new(index));
    let count = sync(&chain, &index).await?;
    let (events, requests) = {
        let mut index = index.lock().unwrap();
        index.log_events = true; // from now on, log every new event
        index.counts()
    };
    info!("rebuilt history from {count} transaction(s): {events} events, {requests} requests");

    let poller = {
        let index = index.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
                match sync(&chain, &index).await {
                    Ok(0) => {}
                    Ok(n) => info!("indexed {n} new transaction(s)"),
                    Err(e) => warn!("sync failed: {e:#}"),
                }
            }
        })
    };
    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port)).await?;
    info!("indexer API on {}", listener.local_addr()?);
    axum::serve(listener, api::router(index)).await?;
    poller.abort();
    Ok(())
}
