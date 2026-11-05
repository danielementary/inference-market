use anchor_lang::prelude::*;

use crate::{error::EscrowError, events::ConfigInitialized, program::InferenceEscrow, state::*};

#[event_cpi]
#[derive(Accounts)]
pub struct InitConfig<'info> {
    /// Must be the program's upgrade authority, so nobody can front-run the deployer.
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        init,
        payer = authority,
        space = DISCRIMINATOR_LEN + Config::INIT_SPACE,
        seeds = [Config::SEED],
        bump,
    )]
    pub config: Account<'info, Config>,
    // Named `escrow_program` because `#[event_cpi]` already adds a `program` field.
    #[account(constraint = escrow_program.programdata_address()? == Some(program_data.key()))]
    pub escrow_program: Program<'info, InferenceEscrow>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()))]
    pub program_data: Account<'info, ProgramData>,
    pub system_program: Program<'info, System>,
}

pub fn handle_init_config(
    ctx: Context<InitConfig>,
    coordinator: Pubkey,
    fee_bps: u16,
    request_fee: u64,
) -> Result<()> {
    require!(fee_bps <= Config::MAX_FEE_BPS, EscrowError::InvalidParams);

    ctx.accounts.config.set_inner(Config {
        coordinator,
        fee_bps,
        request_fee,
        bump: ctx.bumps.config,
    });

    emit_cpi!(ConfigInitialized {
        coordinator,
        fee_bps,
        request_fee,
    });
    Ok(())
}
