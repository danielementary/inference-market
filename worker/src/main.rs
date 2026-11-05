use std::time::Duration;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use common::{chain::Chain, client::types::Model, ix, keys, Signer};
use tracing::{info, warn};
use worker::session::{self, Offer};

#[derive(Parser)]
#[command(about = "Simulated Mac worker")]
struct Args {
    /// Keypair name in keys/ (owner of the Worker account).
    #[arg(long)]
    key: String,
    #[arg(long, value_enum)]
    model: ModelArg,
    /// Advertised throughput, in tokens per second.
    #[arg(long)]
    tps: u32,
    /// Lamports per input token.
    #[arg(long)]
    price_in: u64,
    /// Lamports per output token.
    #[arg(long)]
    price_out: u64,
    #[arg(long, default_value = "ws://127.0.0.1:8080/workers/ws")]
    coordinator: String,
    /// Misbehave on every job: slow, then no start, then crash, in a loop.
    #[arg(long)]
    faulty: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum ModelArg {
    Qwen38,
    Llama4,
}

impl From<ModelArg> for Model {
    fn from(model: ModelArg) -> Self {
        match model {
            ModelArg::Qwen38 => Model::Qwen38,
            ModelArg::Llama4 => Model::Llama4,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let args = Args::parse();
    let owner = keys::load(&args.key)?;
    let mut offer = Offer {
        model: args.model.into(),
        tok_per_s: args.tps,
    };

    let chain = Chain::from_env();
    match chain.worker(&owner.pubkey()).await? {
        None => {
            let ix = ix::register_worker(
                &owner.pubkey(),
                offer.model,
                args.tps,
                args.price_in,
                args.price_out,
            );
            info!(
                "registered worker {}",
                chain.send(&[ix], &owner, &[]).await?
            );
        }
        Some(w)
            if (w.model, w.tok_per_s, w.price_in, w.price_out)
                != (offer.model, args.tps, args.price_in, args.price_out) =>
        {
            // No update_worker instruction yet. The coordinator assigns jobs from the on-chain
            // offer, so serve that one.
            warn!(
                model = ?w.model, tps = w.tok_per_s, price_in = w.price_in, price_out = w.price_out,
                "on-chain offer differs from the arguments and cannot be updated yet; serving the on-chain offer"
            );
            offer = Offer {
                model: w.model,
                tok_per_s: w.tok_per_s,
            };
        }
        Some(_) => info!("worker already registered"),
    }

    loop {
        if let Err(e) = session::run(&args.coordinator, &owner, offer, args.faulty).await {
            warn!("disconnected: {e:#}");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
