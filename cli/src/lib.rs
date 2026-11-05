//! Client-side logic shared by the `infer` CLI and the end-to-end scenarios: quotes, saved
//! prices and the coordinator's event stream. No printing here, so a TUI can reuse it.

pub mod defaults;
pub mod quote;
pub mod stream;
