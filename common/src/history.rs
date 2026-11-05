//! Indexer REST responses, shared with the CLI. Addresses are base58 strings, amounts lamports.

use serde::{Deserialize, Serialize};

/// One request as rebuilt from its events: created, then assigned, then settled or cancelled.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestRecord {
    /// Signature of the `create_request` transaction; unique even when a nonce is reused.
    pub id: String,
    pub request: String,
    pub client: String,
    pub nonce: u64,
    pub model: String,
    pub min_tps: u32,
    pub budget: u64,
    pub request_fee: u64,
    /// `open`, `assigned`, `settled` or `cancelled`.
    pub status: String,
    /// `Worker` PDA, once assigned.
    pub worker: Option<String>,
    pub agreed_price_in: Option<u64>,
    pub agreed_price_out: Option<u64>,
    pub in_tokens: Option<u64>,
    pub out_tokens: Option<u64>,
    pub measured_tps: Option<u32>,
    pub due: Option<u64>,
    pub fee: Option<u64>,
    /// Budget returned to the client (rent excluded).
    pub refund: Option<u64>,
    pub created_at: Option<i64>,
    pub closed_at: Option<i64>,
}

/// What a worker earned over its settled jobs.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Earnings {
    pub owner: String,
    pub worker: String,
    pub model: String,
    pub tok_per_s: u32,
    pub price_in: u64,
    pub price_out: u64,
    pub jobs: u64,
    /// Jobs that paid something (the others missed the SLA or produced nothing).
    pub paid_jobs: u64,
    pub in_tokens: u64,
    pub out_tokens: u64,
    /// Sum of `due - fee`: what reached the owner.
    pub earned: u64,
    pub last_job: Option<RequestRecord>,
}
