//! Off-chain copy of the program's payout rule (`settle.rs`), so the coordinator and the CLI can
//! show the amounts without decoding the transaction. The program stays the source of truth.

/// Amount owed out of the budget: agreed prices × attested tokens, capped by the budget, or 0 if
/// the measured throughput missed the minimum. `None` on overflow (the program rejects it).
pub fn compute_due(
    in_tokens: u64,
    out_tokens: u64,
    measured_tps: u32,
    price_in: u64,
    price_out: u64,
    min_tps: u32,
    budget: u64,
) -> Option<u64> {
    if measured_tps < min_tps {
        return Some(0);
    }
    let cost = (in_tokens as u128 * price_in as u128)
        .checked_add(out_tokens as u128 * price_out as u128)?;
    Some(cost.min(budget as u128) as u64)
}

/// Coordinator share of `due`, rounded down.
pub fn compute_fee(due: u64, fee_bps: u16) -> u64 {
    (due as u128 * fee_bps as u128 / 10_000) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    // Same cases as the program's unit tests.
    #[test]
    fn matches_program_rule() {
        assert_eq!(
            compute_due(200, 1_500, 30, 100, 400, 30, 1_000_000),
            Some(620_000)
        );
        assert_eq!(
            compute_due(200, 1_500, 29, 100, 400, 30, 1_000_000),
            Some(0)
        );
        assert_eq!(
            compute_due(200, 1_500, 40, 100, 400, 30, 1_000),
            Some(1_000)
        );
        assert_eq!(
            compute_due(u64::MAX, u64::MAX, 0, u64::MAX, u64::MAX, 0, u64::MAX),
            None
        );
        assert_eq!(compute_fee(620_000, 200), 12_400);
        assert_eq!(compute_fee(99, 200), 1);
    }
}
