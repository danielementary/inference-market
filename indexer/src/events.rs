//! Program events as emitted by `emit_cpi!`: a self-CPI whose data is
//! `EVENT_IX_TAG (8, LE) || sha256("event:<Name>")[..8] || borsh(event)`.
//! The generated client doesn't cover events, so they are declared here, field for field as in
//! `onchain/programs/inference-escrow/src/events.rs`.

use borsh::BorshDeserialize;
use common::client::types::Model;
use sha2::{Digest, Sha256};
use solana_address::Address;

const EVENT_IX_TAG: u64 = 0x1d9a_cb51_2ea5_45e4;

#[derive(BorshDeserialize, Debug, Clone, PartialEq, Eq)]
pub struct ConfigInitialized {
    pub coordinator: Address,
    pub fee_bps: u16,
    pub request_fee: u64,
}

#[derive(BorshDeserialize, Debug, Clone, PartialEq, Eq)]
pub struct WorkerRegistered {
    pub worker: Address,
    pub owner: Address,
    pub model: Model,
    pub tok_per_s: u32,
    pub price_in: u64,
    pub price_out: u64,
}

#[derive(BorshDeserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestCreated {
    pub request: Address,
    pub client: Address,
    pub nonce: u64,
    pub model: Model,
    pub min_tps: u32,
    pub max_price_in: u64,
    pub max_price_out: u64,
    pub budget: u64,
    pub prompt_hash: [u8; 32],
    pub request_fee: u64,
}

#[derive(BorshDeserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestCancelled {
    pub request: Address,
    pub client: Address,
    pub refund: u64,
}

#[derive(BorshDeserialize, Debug, Clone, PartialEq, Eq)]
pub struct WorkerAssigned {
    pub request: Address,
    pub worker: Address,
    pub agreed_price_in: u64,
    pub agreed_price_out: u64,
}

#[derive(BorshDeserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestSettled {
    pub request: Address,
    pub client: Address,
    pub worker: Address,
    pub in_tokens: u64,
    pub out_tokens: u64,
    pub measured_tps: u32,
    pub due: u64,
    pub fee: u64,
    pub refund: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    ConfigInitialized(ConfigInitialized),
    WorkerRegistered(WorkerRegistered),
    RequestCreated(RequestCreated),
    RequestCancelled(RequestCancelled),
    WorkerAssigned(WorkerAssigned),
    RequestSettled(RequestSettled),
}

/// Anchor event discriminator: `sha256("event:<name>")[..8]`.
pub fn discriminator(name: &str) -> [u8; 8] {
    let hash = Sha256::digest(format!("event:{name}"));
    hash[..8].try_into().expect("8 bytes")
}

impl Event {
    pub fn name(&self) -> &'static str {
        match self {
            Event::ConfigInitialized(_) => "ConfigInitialized",
            Event::WorkerRegistered(_) => "WorkerRegistered",
            Event::RequestCreated(_) => "RequestCreated",
            Event::RequestCancelled(_) => "RequestCancelled",
            Event::WorkerAssigned(_) => "WorkerAssigned",
            Event::RequestSettled(_) => "RequestSettled",
        }
    }

    /// Decodes the data of an `emit_cpi!` instruction; `None` for anything else.
    pub fn decode(data: &[u8]) -> Option<Event> {
        let data = data.strip_prefix(&EVENT_IX_TAG.to_le_bytes())?;
        let (disc, mut body) = data.split_at_checked(8)?;
        let body = &mut body;
        let event = match disc {
            d if d == discriminator("ConfigInitialized") => {
                Event::ConfigInitialized(BorshDeserialize::deserialize(body).ok()?)
            }
            d if d == discriminator("WorkerRegistered") => {
                Event::WorkerRegistered(BorshDeserialize::deserialize(body).ok()?)
            }
            d if d == discriminator("RequestCreated") => {
                Event::RequestCreated(BorshDeserialize::deserialize(body).ok()?)
            }
            d if d == discriminator("RequestCancelled") => {
                Event::RequestCancelled(BorshDeserialize::deserialize(body).ok()?)
            }
            d if d == discriminator("WorkerAssigned") => {
                Event::WorkerAssigned(BorshDeserialize::deserialize(body).ok()?)
            }
            d if d == discriminator("RequestSettled") => {
                Event::RequestSettled(BorshDeserialize::deserialize(body).ok()?)
            }
            _ => return None,
        };
        Some(event)
    }

    /// JSON payload stored with the raw event log.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Event::ConfigInitialized(e) => json!({
                "coordinator": e.coordinator.to_string(), "fee_bps": e.fee_bps, "request_fee": e.request_fee,
            }),
            Event::WorkerRegistered(e) => json!({
                "worker": e.worker.to_string(), "owner": e.owner.to_string(), "model": format!("{:?}", e.model),
                "tok_per_s": e.tok_per_s, "price_in": e.price_in, "price_out": e.price_out,
            }),
            Event::RequestCreated(e) => json!({
                "request": e.request.to_string(), "client": e.client.to_string(), "nonce": e.nonce,
                "model": format!("{:?}", e.model), "min_tps": e.min_tps, "max_price_in": e.max_price_in,
                "max_price_out": e.max_price_out, "budget": e.budget, "request_fee": e.request_fee,
            }),
            Event::RequestCancelled(e) => json!({
                "request": e.request.to_string(), "client": e.client.to_string(), "refund": e.refund,
            }),
            Event::WorkerAssigned(e) => json!({
                "request": e.request.to_string(), "worker": e.worker.to_string(),
                "agreed_price_in": e.agreed_price_in, "agreed_price_out": e.agreed_price_out,
            }),
            Event::RequestSettled(e) => json!({
                "request": e.request.to_string(), "client": e.client.to_string(), "worker": e.worker.to_string(),
                "in_tokens": e.in_tokens, "out_tokens": e.out_tokens, "measured_tps": e.measured_tps,
                "due": e.due, "fee": e.fee, "refund": e.refund,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use borsh::BorshSerialize;

    use super::*;

    #[derive(BorshSerialize)]
    struct Cancelled {
        request: [u8; 32],
        client: [u8; 32],
        refund: u64,
    }

    fn cpi_data(name: &str, body: impl BorshSerialize) -> Vec<u8> {
        [
            EVENT_IX_TAG.to_le_bytes().to_vec(),
            discriminator(name).to_vec(),
            borsh::to_vec(&body).unwrap(),
        ]
        .concat()
    }

    #[test]
    fn decodes_emit_cpi_data() {
        let data = cpi_data(
            "RequestCancelled",
            Cancelled {
                request: [1; 32],
                client: [2; 32],
                refund: 202,
            },
        );
        let Some(Event::RequestCancelled(e)) = Event::decode(&data) else {
            panic!("not decoded")
        };
        assert_eq!(
            (e.request, e.client, e.refund),
            (Address::from([1; 32]), Address::from([2; 32]), 202)
        );
    }

    #[test]
    fn ignores_other_instructions() {
        assert_eq!(Event::decode(b"not an event"), None);
        let unknown = cpi_data("SomethingElse", 1u8);
        assert_eq!(Event::decode(&unknown), None);
        let truncated = cpi_data("RequestCancelled", 7u8);
        assert_eq!(Event::decode(&truncated), None);
    }

    /// The hand-written discriminators must match the ones Anchor put in the IDL.
    #[test]
    fn discriminators_match_idl() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../onchain/target/idl/inference_escrow.json"
        );
        let Ok(idl) = std::fs::read_to_string(path) else {
            eprintln!("IDL not built, skipping");
            return;
        };
        let idl: serde_json::Value = serde_json::from_str(&idl).unwrap();
        let events = idl["events"].as_array().unwrap();
        assert_eq!(events.len(), 6);
        for event in events {
            let name = event["name"].as_str().unwrap();
            let expected: Vec<u8> = serde_json::from_value(event["discriminator"].clone()).unwrap();
            assert_eq!(discriminator(name).to_vec(), expected, "{name}");
        }
    }
}
