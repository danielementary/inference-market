use anchor_lang::prelude::*;

/// Anchor account discriminator size, prepended to every account.
pub const DISCRIMINATOR_LEN: usize = 8;

/// Global settings. One instance, immutable after `init_config`.
#[account]
#[derive(InitSpace)]
pub struct Config {
    /// Only key allowed to sign `assign_worker` and `settle`; also receives the fee.
    pub coordinator: Pubkey,
    /// Share of each payment kept by the coordinator, in basis points (1 bps = 0.01%).
    pub fee_bps: u16,
    /// Flat lamports paid by the client to the coordinator at `create_request`. Never refunded,
    /// so spamming and cancelling requests has a cost.
    pub request_fee: u64,
    pub bump: u8,
}

impl Config {
    pub const SEED: &'static [u8] = b"config";
    /// Upper bound accepted by `init_config` (10%).
    pub const MAX_FEE_BPS: u16 = 1_000;
}

/// Supported models. Adding one requires a program upgrade; only append variants,
/// since the variant index is the on-chain encoding.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum Model {
    Qwen38,
    Llama4,
}

/// A worker's offer: one model at a time, with its advertised throughput and prices.
#[account]
#[derive(InitSpace)]
pub struct Worker {
    /// Signs updates and receives payments.
    pub owner: Pubkey,
    pub model: Model,
    /// Advertised throughput, compared to `Request::min_tps` at assignment.
    pub tok_per_s: u32,
    /// Lamports per input token.
    pub price_in: u64,
    /// Lamports per output token.
    pub price_out: u64,
    /// Accepts new jobs.
    pub active: bool,
    pub bump: u8,
}

impl Worker {
    pub const SEED: &'static [u8] = b"worker";
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum RequestStatus {
    Open,
    Assigned,
}

/// An inference request. The account itself is the escrow: lamports = rent + `budget`.
/// Closed to `client` by `settle` or `cancel_request`.
#[account]
#[derive(InitSpace)]
pub struct Request {
    /// Requester; receives the unspent budget and the rent on close.
    pub client: Pubkey,
    /// Chosen by the client; can be reused once the account is closed.
    pub nonce: u64,
    pub model: Model,
    /// Minimum throughput (tokens/s) the client accepts.
    pub min_tps: u32,
    /// Maximum accepted lamports per input token.
    pub max_price_in: u64,
    /// Maximum accepted lamports per output token.
    pub max_price_out: u64,
    /// Hard cap on the payment, held in escrow. Never derived from the account balance.
    pub budget: u64,
    /// `sha256(salt || prompt)`; salt and prompt stay off-chain.
    pub prompt_hash: [u8; 32],
    pub status: RequestStatus,
    /// Assigned `Worker` PDA; default until assigned.
    pub worker: Pubkey,
    /// Worker prices frozen at assignment.
    pub agreed_price_in: u64,
    pub agreed_price_out: u64,
    pub bump: u8,
}

impl Request {
    pub const SEED: &'static [u8] = b"request";
}

// Sizes listed in the README's accounts table.
const _: () = assert!(DISCRIMINATOR_LEN + Config::INIT_SPACE == 51);
const _: () = assert!(DISCRIMINATOR_LEN + Worker::INIT_SPACE == 63);
const _: () = assert!(DISCRIMINATOR_LEN + Request::INIT_SPACE == 159);
