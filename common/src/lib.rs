//! Shared off-chain code: RPC access, ready-to-send instructions, prompt hashing, keys, the
//! payout rule and the coordinator protocols (worker WebSocket, client HTTP/SSE).

pub mod chain;
pub mod history;
pub mod ix;
pub mod keys;
pub mod pricing;
pub mod prompt;
pub mod protocol;

pub use inference_escrow_client as client;
pub use solana_address::Address;
pub use solana_keypair::Keypair;
pub use solana_signer::Signer;
