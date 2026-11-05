use anchor_lang::prelude::*;

use crate::{error::EscrowError, events::WorkerAssigned, state::*};

#[event_cpi]
#[derive(Accounts)]
pub struct AssignWorker<'info> {
    pub coordinator: Signer<'info>,
    #[account(seeds = [Config::SEED], bump = config.bump, has_one = coordinator)]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        constraint = request.status == RequestStatus::Open @ EscrowError::InvalidStatus,
    )]
    pub request: Account<'info, Request>,
    pub worker: Account<'info, Worker>,
}

pub fn handle_assign_worker(ctx: Context<AssignWorker>) -> Result<()> {
    let worker = &ctx.accounts.worker;
    let request = &mut ctx.accounts.request;

    require!(worker.active, EscrowError::WorkerInactive);
    require!(worker.model == request.model, EscrowError::ModelMismatch);
    require!(
        worker.tok_per_s >= request.min_tps,
        EscrowError::ThroughputTooLow
    );
    require!(
        worker.price_in <= request.max_price_in && worker.price_out <= request.max_price_out,
        EscrowError::PriceAboveMax
    );

    // Freeze the worker's prices: later offer changes don't affect this request.
    request.status = RequestStatus::Assigned;
    request.worker = worker.key();
    request.agreed_price_in = worker.price_in;
    request.agreed_price_out = worker.price_out;

    emit_cpi!(WorkerAssigned {
        request: ctx.accounts.request.key(),
        worker: ctx.accounts.worker.key(),
        agreed_price_in: ctx.accounts.request.agreed_price_in,
        agreed_price_out: ctx.accounts.request.agreed_price_out,
    });
    Ok(())
}
