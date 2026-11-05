use anchor_lang::prelude::*;

use crate::state::Model;

// Each event carries the values actually applied, so the indexer can rebuild history
// from transactions alone, without reading accounts.

#[event]
pub struct ConfigInitialized {
    pub coordinator: Pubkey,
    pub fee_bps: u16,
    pub request_fee: u64,
}

#[event]
pub struct WorkerRegistered {
    pub worker: Pubkey,
    pub owner: Pubkey,
    pub model: Model,
    pub tok_per_s: u32,
    pub price_in: u64,
    pub price_out: u64,
}

#[event]
pub struct RequestCreated {
    pub request: Pubkey,
    pub client: Pubkey,
    pub nonce: u64,
    pub model: Model,
    pub min_tps: u32,
    pub max_price_in: u64,
    pub max_price_out: u64,
    pub budget: u64,
    pub prompt_hash: [u8; 32],
    pub request_fee: u64,
}

#[event]
pub struct RequestCancelled {
    pub request: Pubkey,
    pub client: Pubkey,
    /// Budget returned to the client (rent excluded; `request_fee` is never refunded).
    pub refund: u64,
}

#[event]
pub struct WorkerAssigned {
    pub request: Pubkey,
    pub worker: Pubkey,
    pub agreed_price_in: u64,
    pub agreed_price_out: u64,
}

#[event]
pub struct RequestSettled {
    pub request: Pubkey,
    pub client: Pubkey,
    pub worker: Pubkey,
    pub in_tokens: u64,
    pub out_tokens: u64,
    pub measured_tps: u32,
    /// Total paid out of the budget: `worker_amount + fee`.
    pub due: u64,
    pub fee: u64,
    /// Budget returned to the client (rent excluded).
    pub refund: u64,
}
