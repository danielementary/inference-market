//! Rebuilds history from the program's transactions: signatures newer than the last one
//! indexed (all of them on start), oldest first, each decoded into the events its `emit_cpi!`
//! self-CPIs carry.

use std::sync::Mutex;

use anyhow::{Context, Result};
use common::{chain::Chain, client::ID};
use solana_commitment_config::CommitmentConfig;
use solana_rpc_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_rpc_client_api::config::RpcTransactionConfig;
use solana_signature::Signature;
use solana_transaction_status_client_types::{
    option_serializer::OptionSerializer, EncodedTransaction, UiInstruction, UiMessage,
    UiTransactionEncoding,
};

use crate::{
    events::Event,
    index::{Index, IndexedTx},
};

const PAGE: usize = 1_000;

/// Indexes every program transaction not seen yet. Returns how many were processed.
pub async fn sync(chain: &Chain, index: &Mutex<Index>) -> Result<usize> {
    let until = index
        .lock()
        .unwrap()
        .last_signature()
        .map(|s| s.parse::<Signature>())
        .transpose()?;

    // The RPC returns newest first, in pages; walk back to the last signature indexed.
    let mut signatures = Vec::new();
    let mut before = None;
    loop {
        let config = GetConfirmedSignaturesForAddress2Config {
            before,
            until,
            limit: Some(PAGE),
            commitment: Some(CommitmentConfig::confirmed()),
        };
        let page = chain
            .rpc
            .get_signatures_for_address_with_config(&ID, config)
            .await?;
        let full = page.len() == PAGE;
        if let Some(last) = page.last() {
            before = Some(last.signature.parse()?);
        }
        signatures.extend(page);
        if !full {
            break;
        }
    }

    let count = signatures.len();
    for status in signatures.into_iter().rev() {
        // Failed transactions emitted nothing; they still advance the last signature.
        let tx = if status.err.is_some() {
            IndexedTx {
                signature: status.signature,
                slot: status.slot,
                block_time: status.block_time,
                events: vec![],
            }
        } else {
            fetch(chain, &status.signature).await?
        };
        index.lock().unwrap().apply(&tx);
    }
    Ok(count)
}

/// Loads one transaction and decodes the events of this program's inner instructions, in order.
async fn fetch(chain: &Chain, signature: &str) -> Result<IndexedTx> {
    let config = RpcTransactionConfig {
        encoding: Some(UiTransactionEncoding::Json),
        commitment: Some(CommitmentConfig::confirmed()),
        max_supported_transaction_version: Some(0),
    };
    let tx = chain
        .rpc
        .get_transaction_with_config(&signature.parse()?, config)
        .await
        .with_context(|| format!("fetching {signature}"))?;

    let EncodedTransaction::Json(ui) = &tx.transaction.transaction else {
        anyhow::bail!("{signature}: unexpected transaction encoding");
    };
    let UiMessage::Raw(message) = &ui.message else {
        anyhow::bail!("{signature}: unexpected message encoding");
    };
    let program = ID.to_string();

    let mut events = Vec::new();
    let meta = tx.transaction.meta.context("missing transaction meta")?;
    if meta.err.is_none() {
        if let OptionSerializer::Some(mut inner) = meta.inner_instructions {
            inner.sort_by_key(|i| i.index);
            for ix in inner.iter().flat_map(|i| &i.instructions) {
                let UiInstruction::Compiled(ix) = ix else {
                    continue;
                };
                // Only this program's self-CPIs carry its events.
                if message.account_keys.get(ix.program_id_index as usize) != Some(&program) {
                    continue;
                }
                let data = bs58::decode(&ix.data).into_vec()?;
                events.extend(Event::decode(&data));
            }
        }
    }
    Ok(IndexedTx {
        signature: signature.into(),
        slot: tx.slot,
        block_time: tx.block_time,
        events,
    })
}
