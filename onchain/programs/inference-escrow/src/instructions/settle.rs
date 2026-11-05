use anchor_lang::prelude::*;

use crate::{error::EscrowError, events::RequestSettled, state::*};

#[event_cpi]
#[derive(Accounts)]
pub struct Settle<'info> {
    /// Signs the attested usage and receives the fee.
    #[account(mut)]
    pub coordinator: Signer<'info>,
    #[account(seeds = [Config::SEED], bump = config.bump, has_one = coordinator)]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        has_one = client,
        has_one = worker,
        close = client,
        constraint = request.status == RequestStatus::Assigned @ EscrowError::InvalidStatus,
    )]
    pub request: Account<'info, Request>,
    #[account(has_one = owner)]
    pub worker: Account<'info, Worker>,
    /// Worker's owner, paid directly (push model).
    #[account(mut)]
    pub owner: SystemAccount<'info>,
    /// Receives the unspent budget and the rent on close.
    #[account(mut)]
    pub client: SystemAccount<'info>,
}

/// Amount owed out of the budget: the agreed prices times the attested tokens, capped by the
/// budget, or nothing if the measured throughput missed the minimum.
pub fn compute_due(
    request: &Request,
    in_tokens: u64,
    out_tokens: u64,
    measured_tps: u32,
) -> Result<u64> {
    if measured_tps < request.min_tps {
        return Ok(0);
    }
    let cost = (in_tokens as u128 * request.agreed_price_in as u128)
        .checked_add(out_tokens as u128 * request.agreed_price_out as u128)
        .ok_or(EscrowError::MathOverflow)?;
    Ok(cost.min(request.budget as u128) as u64)
}

/// Coordinator share of `due`, rounded down. Never exceeds `due` since `fee_bps <= 10_000`.
pub fn compute_fee(due: u64, fee_bps: u16) -> u64 {
    (due as u128 * fee_bps as u128 / 10_000) as u64
}

pub fn handle_settle(
    ctx: Context<Settle>,
    in_tokens: u64,
    out_tokens: u64,
    measured_tps: u32,
) -> Result<()> {
    let budget = ctx.accounts.request.budget;
    let due = compute_due(&ctx.accounts.request, in_tokens, out_tokens, measured_tps)?;
    let fee = compute_fee(due, ctx.accounts.config.fee_bps);

    // The request is program-owned, so its lamports are debited directly (no CPI). The rest
    // (budget - due + rent) goes back to the client when the account closes.
    ctx.accounts.request.sub_lamports(due)?;
    ctx.accounts.owner.add_lamports(due - fee)?;
    ctx.accounts.coordinator.add_lamports(fee)?;

    emit_cpi!(RequestSettled {
        request: ctx.accounts.request.key(),
        client: ctx.accounts.client.key(),
        worker: ctx.accounts.worker.key(),
        in_tokens,
        out_tokens,
        measured_tps,
        due,
        fee,
        refund: budget - due,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(min_tps: u32, budget: u64, price_in: u64, price_out: u64) -> Request {
        Request {
            client: Pubkey::default(),
            nonce: 0,
            model: Model::Qwen38,
            min_tps,
            max_price_in: price_in,
            max_price_out: price_out,
            budget,
            prompt_hash: [0; 32],
            status: RequestStatus::Assigned,
            worker: Pubkey::default(),
            agreed_price_in: price_in,
            agreed_price_out: price_out,
            bump: 0,
        }
    }

    #[test]
    fn due_is_tokens_times_agreed_prices() {
        let r = request(30, 1_000_000, 100, 400);
        assert_eq!(compute_due(&r, 200, 1_500, 30).unwrap(), 620_000);
    }

    #[test]
    fn due_is_zero_below_min_tps() {
        let r = request(30, 1_000_000, 100, 400);
        assert_eq!(compute_due(&r, 200, 1_500, 29).unwrap(), 0);
    }

    #[test]
    fn due_is_capped_at_budget() {
        let r = request(30, 1_000, 100, 400);
        assert_eq!(compute_due(&r, 200, 1_500, 40).unwrap(), 1_000);
    }

    #[test]
    fn due_overflow_is_rejected() {
        let r = request(0, u64::MAX, u64::MAX, u64::MAX);
        assert!(compute_due(&r, u64::MAX, u64::MAX, 0).is_err());
    }

    #[test]
    fn fee_rounds_down() {
        assert_eq!(compute_fee(620_000, 200), 12_400);
        assert_eq!(compute_fee(99, 200), 1);
        assert_eq!(compute_fee(49, 200), 0);
        assert_eq!(compute_fee(u64::MAX, 10_000), u64::MAX);
    }
}
