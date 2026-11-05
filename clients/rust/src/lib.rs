//! Rust client for the `inference_escrow` program.
//!
//! Everything under `generated/` is produced by Codama from
//! `onchain/target/idl/inference_escrow.json`; regenerate with `scripts/generate-client.sh`.

mod generated;

pub use generated::programs::INFERENCE_ESCROW_ID as ID;
pub use generated::*;
