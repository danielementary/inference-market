//! Pure matching and metering rules.

use std::time::Duration;

use common::client::accounts::{Request, Worker};

/// Minimum output the budget must cover, on top of the prompt, before a job is assigned.
pub const MIN_OUT_TOKENS: u64 = 16;

/// Cost of the prompt plus the minimum output at this worker's prices.
fn minimum_cost(worker: &Worker, in_tokens: u64) -> u128 {
    in_tokens as u128 * worker.price_in as u128 + MIN_OUT_TOKENS as u128 * worker.price_out as u128
}

/// Same checks as `assign_worker` on-chain, plus a budget large enough for the prompt and a
/// minimum answer.
pub fn compatible(worker: &Worker, request: &Request, in_tokens: u64) -> bool {
    worker.active
        && worker.model == request.model
        && worker.tok_per_s >= request.min_tps
        && worker.price_in <= request.max_price_in
        && worker.price_out <= request.max_price_out
        && minimum_cost(worker, in_tokens) <= request.budget as u128
}

/// Picks the cheapest worker for this prompt among `candidates`.
pub fn cheapest<'a, K: Copy>(
    candidates: impl Iterator<Item = (K, &'a Worker)>,
    in_tokens: u64,
) -> Option<K> {
    candidates
        .min_by_key(|(_, worker)| minimum_cost(worker, in_tokens))
        .map(|(key, _)| key)
}

/// Largest output the budget still pays for after the prompt, at the agreed prices.
pub fn max_out_tokens(budget: u64, in_tokens: u64, price_in: u64, price_out: u64) -> u64 {
    let remaining = (budget as u128).saturating_sub(in_tokens as u128 * price_in as u128);
    match price_out {
        0 => u64::MAX,
        _ => (remaining / price_out as u128).min(u64::MAX as u128) as u64,
    }
}

/// Throughput observed by the coordinator: tokens received over the time from sending the job
/// to the last token, rounded. Zero when nothing arrived.
pub fn measured_tps(out_tokens: u64, elapsed: Duration) -> u32 {
    if out_tokens == 0 || elapsed.is_zero() {
        return 0;
    }
    (out_tokens as f64 / elapsed.as_secs_f64()).round() as u32
}

#[cfg(test)]
mod tests {
    use common::client::types::Model;

    use super::*;

    fn worker(model: Model, tok_per_s: u32, price_in: u64, price_out: u64) -> Worker {
        Worker {
            discriminator: Default::default(),
            owner: Default::default(),
            model,
            tok_per_s,
            price_in,
            price_out,
            active: true,
            bump: 0,
        }
    }

    fn request(budget: u64) -> Request {
        Request {
            discriminator: Default::default(),
            client: Default::default(),
            nonce: 0,
            model: Model::Qwen38,
            min_tps: 30,
            max_price_in: 150,
            max_price_out: 500,
            budget,
            prompt_hash: [0; 32],
            status: common::client::types::RequestStatus::Open,
            worker: Default::default(),
            agreed_price_in: 0,
            agreed_price_out: 0,
            bump: 0,
        }
    }

    #[test]
    fn compatibility_mirrors_assign_checks() {
        let r = request(1_000_000);
        assert!(compatible(&worker(Model::Qwen38, 40, 100, 400), &r, 10));
        assert!(!compatible(&worker(Model::Llama4, 40, 100, 400), &r, 10));
        assert!(!compatible(&worker(Model::Qwen38, 29, 100, 400), &r, 10));
        assert!(!compatible(&worker(Model::Qwen38, 40, 151, 400), &r, 10));
        assert!(!compatible(&worker(Model::Qwen38, 40, 100, 501), &r, 10));
        let mut inactive = worker(Model::Qwen38, 40, 100, 400);
        inactive.active = false;
        assert!(!compatible(&inactive, &r, 10));
    }

    #[test]
    fn budget_must_cover_prompt_and_minimum_output() {
        // 10 × 100 + 16 × 400 = 7 400 lamports.
        let w = worker(Model::Qwen38, 40, 100, 400);
        assert!(compatible(&w, &request(7_400), 10));
        assert!(!compatible(&w, &request(7_399), 10));
    }

    #[test]
    fn picks_cheapest() {
        let a = worker(Model::Qwen38, 40, 100, 400);
        let b = worker(Model::Qwen38, 40, 100, 300);
        assert_eq!(cheapest([(1, &a), (2, &b)].into_iter(), 10), Some(2));
        assert_eq!(cheapest(std::iter::empty::<(u8, &Worker)>(), 10), None);
    }

    #[test]
    fn max_out_fits_remaining_budget() {
        assert_eq!(max_out_tokens(1_000_000, 200, 100, 400), 2_450);
        assert_eq!(max_out_tokens(1_000, 200, 100, 400), 0);
        assert_eq!(max_out_tokens(1_000, 0, 0, 0), u64::MAX);
    }

    #[test]
    fn tps_is_rounded_rate() {
        assert_eq!(measured_tps(70, Duration::from_millis(1_750)), 40);
        assert_eq!(measured_tps(70, Duration::from_millis(1_790)), 39);
        assert_eq!(measured_tps(0, Duration::from_secs(1)), 0);
        assert_eq!(measured_tps(5, Duration::ZERO), 0);
    }
}
