use anchor_lang::{
    prelude::*,
    system_program::{transfer, Transfer},
};

use crate::{error::EscrowError, events::RequestCreated, state::*};

#[event_cpi]
#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct CreateRequest<'info> {
    #[account(mut)]
    pub client: Signer<'info>,
    #[account(seeds = [Config::SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// Receives the non-refundable `request_fee`.
    #[account(mut, address = config.coordinator)]
    pub coordinator: SystemAccount<'info>,
    #[account(
        init,
        payer = client,
        space = DISCRIMINATOR_LEN + Request::INIT_SPACE,
        seeds = [Request::SEED, client.key().as_ref(), &nonce.to_le_bytes()],
        bump,
    )]
    pub request: Account<'info, Request>,
    pub system_program: Program<'info, System>,
}

#[allow(clippy::too_many_arguments)]
pub fn handle_create_request(
    ctx: Context<CreateRequest>,
    nonce: u64,
    model: Model,
    min_tps: u32,
    max_price_in: u64,
    max_price_out: u64,
    budget: u64,
    prompt_hash: [u8; 32],
) -> Result<()> {
    require!(budget > 0, EscrowError::InvalidParams);

    let client = ctx.accounts.client.key();
    ctx.accounts.request.set_inner(Request {
        client,
        nonce,
        model,
        min_tps,
        max_price_in,
        max_price_out,
        budget,
        prompt_hash,
        status: RequestStatus::Open,
        worker: Pubkey::default(),
        agreed_price_in: 0,
        agreed_price_out: 0,
        bump: ctx.bumps.request,
    });

    // Escrow the budget in the request account itself.
    transfer(
        CpiContext::new(
            ctx.accounts.system_program.key(),
            Transfer {
                from: ctx.accounts.client.to_account_info(),
                to: ctx.accounts.request.to_account_info(),
            },
        ),
        budget,
    )?;

    let request_fee = ctx.accounts.config.request_fee;
    if request_fee > 0 {
        transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                Transfer {
                    from: ctx.accounts.client.to_account_info(),
                    to: ctx.accounts.coordinator.to_account_info(),
                },
            ),
            request_fee,
        )?;
    }

    emit_cpi!(RequestCreated {
        request: ctx.accounts.request.key(),
        client,
        nonce,
        model,
        min_tps,
        max_price_in,
        max_price_out,
        budget,
        prompt_hash,
        request_fee,
    });
    Ok(())
}
