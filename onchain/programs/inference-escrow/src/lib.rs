pub mod error;
pub mod events;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use instructions::*;
pub use state::*;

declare_id!("2n8oR4r8QRg71NJMxXuvpYDDmnceUzkZYTLSBivjHAGG");

#[program]
pub mod inference_escrow {
    use super::*;

    pub fn init_config(
        ctx: Context<InitConfig>,
        coordinator: Pubkey,
        fee_bps: u16,
        request_fee: u64,
    ) -> Result<()> {
        handle_init_config(ctx, coordinator, fee_bps, request_fee)
    }

    pub fn register_worker(
        ctx: Context<RegisterWorker>,
        model: Model,
        tok_per_s: u32,
        price_in: u64,
        price_out: u64,
    ) -> Result<()> {
        handle_register_worker(ctx, model, tok_per_s, price_in, price_out)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_request(
        ctx: Context<CreateRequest>,
        nonce: u64,
        model: Model,
        min_tps: u32,
        max_price_in: u64,
        max_price_out: u64,
        budget: u64,
        prompt_hash: [u8; 32],
    ) -> Result<()> {
        handle_create_request(
            ctx,
            nonce,
            model,
            min_tps,
            max_price_in,
            max_price_out,
            budget,
            prompt_hash,
        )
    }

    pub fn cancel_request(ctx: Context<CancelRequest>) -> Result<()> {
        handle_cancel_request(ctx)
    }

    pub fn assign_worker(ctx: Context<AssignWorker>) -> Result<()> {
        handle_assign_worker(ctx)
    }

    pub fn settle(
        ctx: Context<Settle>,
        in_tokens: u64,
        out_tokens: u64,
        measured_tps: u32,
    ) -> Result<()> {
        handle_settle(ctx, in_tokens, out_tokens, measured_tps)
    }
}
