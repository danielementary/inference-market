//! What a request will cost before it is created.

use anyhow::{bail, Context, Result};
use common::client::{accounts::Worker, types::Model};

use crate::defaults::Prices;

/// Where the max prices of a quote come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceSource {
    Options,
    LastUsed,
    /// Highest prices among the compatible offers, so all of them stay eligible.
    Offers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub prices: Prices,
    pub source: PriceSource,
    pub in_tokens: u64,
    pub max_out: u64,
    pub budget: u64,
    /// Active offers for the model and throughput that accept these prices.
    pub eligible: usize,
}

/// Active offers serving `model` at `min_tps` or more.
pub fn matching_offers(offers: &[Worker], model: Model, min_tps: u32) -> Vec<&Worker> {
    offers
        .iter()
        .filter(|w| w.active && w.model == model && w.tok_per_s >= min_tps)
        .collect()
}

/// Prices from the options, else the last ones used, else the highest matching offer prices.
/// `budget`, when given, overrides the computed one.
#[allow(clippy::too_many_arguments)]
pub fn quote(
    offers: &[Worker],
    model: Model,
    min_tps: u32,
    in_tokens: u64,
    max_out: u64,
    options: Option<Prices>,
    last_used: Option<Prices>,
    budget: Option<u64>,
) -> Result<Quote> {
    let matching = matching_offers(offers, model, min_tps);
    if matching.is_empty() {
        bail!("no active {model:?} offer at {min_tps} tok/s or more");
    }
    let (prices, source) = match (options, last_used) {
        (Some(p), _) => (p, PriceSource::Options),
        (None, Some(p)) => (p, PriceSource::LastUsed),
        (None, None) => (
            Prices {
                max_price_in: matching
                    .iter()
                    .map(|w| w.price_in)
                    .max()
                    .expect("non-empty"),
                max_price_out: matching
                    .iter()
                    .map(|w| w.price_out)
                    .max()
                    .expect("non-empty"),
            },
            PriceSource::Offers,
        ),
    };
    let eligible = matching
        .iter()
        .filter(|w| w.price_in <= prices.max_price_in && w.price_out <= prices.max_price_out)
        .count();
    let budget = match budget {
        Some(b) => b,
        None => (in_tokens as u128 * prices.max_price_in as u128
            + max_out as u128 * prices.max_price_out as u128)
            .try_into()
            .ok()
            .context("budget overflows u64")?,
    };
    if budget == 0 {
        bail!("budget is 0");
    }
    Ok(Quote {
        prices,
        source,
        in_tokens,
        max_out,
        budget,
        eligible,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(model: Model, tok_per_s: u32, price_in: u64, price_out: u64) -> Worker {
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

    fn offers() -> Vec<Worker> {
        vec![
            offer(Model::Qwen38, 40, 100, 400),
            offer(Model::Qwen38, 60, 120, 350),
            offer(Model::Qwen38, 10, 500, 900), // too slow for 30 tok/s
            offer(Model::Llama4, 30, 80, 300),
        ]
    }

    #[test]
    fn defaults_to_highest_matching_prices() {
        let q = quote(&offers(), Model::Qwen38, 30, 10, 100, None, None, None).unwrap();
        assert_eq!(
            q.prices,
            Prices {
                max_price_in: 120,
                max_price_out: 400
            }
        );
        assert_eq!(q.source, PriceSource::Offers);
        assert_eq!(q.budget, 10 * 120 + 100 * 400);
        assert_eq!(q.eligible, 2);
    }

    #[test]
    fn options_beat_last_used() {
        let opts = Prices {
            max_price_in: 100,
            max_price_out: 400,
        };
        let last = Prices {
            max_price_in: 1,
            max_price_out: 1,
        };
        let q = quote(
            &offers(),
            Model::Qwen38,
            30,
            10,
            100,
            Some(opts),
            Some(last),
            None,
        )
        .unwrap();
        assert_eq!(
            (q.prices, q.source, q.eligible),
            (opts, PriceSource::Options, 1)
        );
        let q = quote(
            &offers(),
            Model::Qwen38,
            30,
            10,
            100,
            None,
            Some(last),
            None,
        )
        .unwrap();
        assert_eq!(
            (q.prices, q.source, q.eligible),
            (last, PriceSource::LastUsed, 0)
        );
    }

    #[test]
    fn explicit_budget_wins() {
        let q = quote(&offers(), Model::Qwen38, 30, 10, 100, None, None, Some(5)).unwrap();
        assert_eq!(q.budget, 5);
    }

    #[test]
    fn no_matching_offer_is_an_error() {
        assert!(quote(&offers(), Model::Llama4, 31, 10, 100, None, None, None).is_err());
    }
}
