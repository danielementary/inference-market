//! End-to-end check against a running validator (`scripts/localnet.sh`):
//! init_config (once) → register_worker (once) → create_request → assign_worker → settle.
//!
//! Run from the repo root: `cargo run -p common --example smoke`

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use common::{
    chain::Chain,
    client::{instructions::CreateRequestInstructionArgs, types::Model},
    ix::{self, Usage},
    keys, prompt, Signer,
};

#[tokio::main]
async fn main() -> Result<()> {
    let chain = Chain::from_env();
    let admin = keys::load("admin")?;
    let coordinator = keys::load("coordinator")?;
    let client = keys::load("client-a")?;
    let owner = keys::load("worker-1")?;

    if chain.config().await?.is_none() {
        let ix = ix::init_config(&admin.pubkey(), coordinator.pubkey(), 200, 10_000);
        println!("init_config      {}", chain.send(&[ix], &admin, &[]).await?);
    }
    if chain.worker(&owner.pubkey()).await?.is_none() {
        let ix = ix::register_worker(&owner.pubkey(), Model::Qwen38, 40, 100, 400);
        println!("register_worker  {}", chain.send(&[ix], &owner, &[]).await?);
    }
    println!("workers: {}", chain.workers().await?.len());

    let before = [
        chain.balance(&client.pubkey()).await?,
        chain.balance(&owner.pubkey()).await?,
        chain.balance(&coordinator.pubkey()).await?,
    ];

    let prompt_text = "Explain Solana rent in one sentence";
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let args = CreateRequestInstructionArgs {
        nonce,
        model: Model::Qwen38,
        min_tps: 30,
        max_price_in: 150,
        max_price_out: 500,
        budget: 1_000_000,
        prompt_hash: prompt::prompt_hash(&prompt::new_salt(), prompt_text),
    };
    let ix = ix::create_request(&client.pubkey(), &coordinator.pubkey(), args);
    println!(
        "create_request   {}",
        chain.send(&[ix], &client, &[]).await?
    );

    let request = ix::request_pda(&client.pubkey(), nonce);
    let ix = ix::assign_worker(&coordinator.pubkey(), request, &owner.pubkey());
    println!(
        "assign_worker    {}",
        chain.send(&[ix], &coordinator, &[]).await?
    );

    let usage = Usage {
        in_tokens: prompt::count_tokens(prompt_text),
        out_tokens: 1_500,
        measured_tps: 40,
    };
    let ix = ix::settle(
        &coordinator.pubkey(),
        &client.pubkey(),
        nonce,
        &owner.pubkey(),
        usage,
    );
    println!(
        "settle           {}",
        chain.send(&[ix], &coordinator, &[]).await?
    );

    assert!(
        chain.request(&client.pubkey(), nonce).await?.is_none(),
        "request should be closed"
    );
    let after = [
        chain.balance(&client.pubkey()).await?,
        chain.balance(&owner.pubkey()).await?,
        chain.balance(&coordinator.pubkey()).await?,
    ];
    // Each party paid its own transaction fees, so these deltas include them.
    for (name, (b, a)) in ["client", "worker-1", "coordinator"]
        .iter()
        .zip(before.iter().zip(after))
    {
        println!("{name:<12} {:+}", a as i128 - *b as i128);
    }
    Ok(())
}
