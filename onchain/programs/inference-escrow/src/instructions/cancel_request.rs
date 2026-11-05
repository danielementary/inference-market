use anchor_lang::prelude::*;

use crate::{error::EscrowError, events::RequestCancelled, state::*};

#[event_cpi]
#[derive(Accounts)]
pub struct CancelRequest<'info> {
    #[account(mut)]
    pub client: Signer<'info>,
    /// Closing returns the budget and the rent; `request_fee` was paid out at creation.
    #[account(
        mut,
        has_one = client,
        close = client,
        constraint = request.status == RequestStatus::Open @ EscrowError::InvalidStatus,
    )]
    pub request: Account<'info, Request>,
}

pub fn handle_cancel_request(ctx: Context<CancelRequest>) -> Result<()> {
    emit_cpi!(RequestCancelled {
        request: ctx.accounts.request.key(),
        client: ctx.accounts.client.key(),
        refund: ctx.accounts.request.budget,
    });
    Ok(())
}
