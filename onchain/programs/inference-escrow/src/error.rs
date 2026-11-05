use anchor_lang::prelude::*;

#[error_code]
pub enum EscrowError {
    #[msg("Invalid instruction parameters")]
    InvalidParams,
    #[msg("Request is not in the expected status")]
    InvalidStatus,
    #[msg("Worker is not active")]
    WorkerInactive,
    #[msg("Worker serves a different model")]
    ModelMismatch,
    #[msg("Worker throughput is below the requested minimum")]
    ThroughputTooLow,
    #[msg("Worker price is above the accepted maximum")]
    PriceAboveMax,
    #[msg("Arithmetic overflow")]
    MathOverflow,
}
