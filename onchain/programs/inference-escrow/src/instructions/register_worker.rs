use anchor_lang::prelude::*;

use crate::{error::EscrowError, events::WorkerRegistered, state::*};

#[event_cpi]
#[derive(Accounts)]
pub struct RegisterWorker<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(
        init,
        payer = owner,
        space = DISCRIMINATOR_LEN + Worker::INIT_SPACE,
        seeds = [Worker::SEED, owner.key().as_ref()],
        bump,
    )]
    pub worker: Account<'info, Worker>,
    pub system_program: Program<'info, System>,
}

pub fn handle_register_worker(
    ctx: Context<RegisterWorker>,
    model: Model,
    tok_per_s: u32,
    price_in: u64,
    price_out: u64,
) -> Result<()> {
    require!(tok_per_s > 0, EscrowError::InvalidParams);

    let owner = ctx.accounts.owner.key();
    ctx.accounts.worker.set_inner(Worker {
        owner,
        model,
        tok_per_s,
        price_in,
        price_out,
        active: true,
        bump: ctx.bumps.worker,
    });

    emit_cpi!(WorkerRegistered {
        worker: ctx.accounts.worker.key(),
        owner,
        model,
        tok_per_s,
        price_in,
        price_out,
    });
    Ok(())
}
