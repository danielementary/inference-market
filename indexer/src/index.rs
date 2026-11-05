//! In-memory index, rebuilt from the chain on every start: the raw event log plus projections
//! (requests, workers). Nothing is persisted; the program's transactions are the source of truth.

use std::collections::{HashMap, HashSet};

use common::history::{Earnings, RequestRecord};
use serde::Serialize;

use crate::events::Event;

/// A Solana transaction of the program, with the events it emitted in order.
pub struct IndexedTx {
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    pub events: Vec<Event>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
struct LoggedEvent {
    signature: String,
    ix: usize,
    slot: u64,
    kind: &'static str,
    data: serde_json::Value,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
struct WorkerRow {
    worker: String,
    owner: String,
    model: String,
    tok_per_s: u32,
    price_in: u64,
    price_out: u64,
}

#[derive(Default)]
pub struct Index {
    /// (signature, position) of every event applied, so a replay changes nothing.
    seen: HashSet<(String, usize)>,
    events: Vec<LoggedEvent>,
    /// By `Worker` PDA.
    workers: HashMap<String, WorkerRow>,
    /// In creation order; a nonce reused after close gets a new record.
    requests: Vec<RequestRecord>,
    /// Request PDA → position in `requests`, while the request is open or assigned.
    live: HashMap<String, usize>,
    /// Newest signature applied; syncing resumes after it.
    last_signature: Option<String>,
    /// Log each newly applied event (off while replaying history, unless asked for).
    pub log_events: bool,
}

impl Index {
    pub fn last_signature(&self) -> Option<&str> {
        self.last_signature.as_deref()
    }

    /// Applies a transaction's events. Events already applied are skipped.
    pub fn apply(&mut self, tx: &IndexedTx) {
        for (ix, event) in tx.events.iter().enumerate() {
            if !self.seen.insert((tx.signature.clone(), ix)) {
                continue;
            }
            let data = event.to_json();
            if self.log_events {
                tracing::info!(sig = %&tx.signature[..8], "{:<17} {data}", event.name());
            }
            self.events.push(LoggedEvent {
                signature: tx.signature.clone(),
                ix,
                slot: tx.slot,
                kind: event.name(),
                data,
            });
            self.project(tx, event);
        }
        self.last_signature = Some(tx.signature.clone());
    }

    fn project(&mut self, tx: &IndexedTx, event: &Event) {
        match event {
            Event::ConfigInitialized(_) => {} // kept in the event log only
            Event::WorkerRegistered(e) => {
                let row = WorkerRow {
                    worker: e.worker.to_string(),
                    owner: e.owner.to_string(),
                    model: format!("{:?}", e.model),
                    tok_per_s: e.tok_per_s,
                    price_in: e.price_in,
                    price_out: e.price_out,
                };
                self.workers.insert(row.worker.clone(), row);
            }
            Event::RequestCreated(e) => {
                self.live.insert(e.request.to_string(), self.requests.len());
                self.requests.push(RequestRecord {
                    id: tx.signature.clone(),
                    request: e.request.to_string(),
                    client: e.client.to_string(),
                    nonce: e.nonce,
                    model: format!("{:?}", e.model),
                    min_tps: e.min_tps,
                    budget: e.budget,
                    request_fee: e.request_fee,
                    status: "open".into(),
                    worker: None,
                    agreed_price_in: None,
                    agreed_price_out: None,
                    in_tokens: None,
                    out_tokens: None,
                    measured_tps: None,
                    due: None,
                    fee: None,
                    refund: None,
                    created_at: tx.block_time,
                    closed_at: None,
                });
            }
            Event::WorkerAssigned(e) => {
                if let Some(r) = self.live_request(&e.request.to_string(), "open") {
                    r.status = "assigned".into();
                    r.worker = Some(e.worker.to_string());
                    r.agreed_price_in = Some(e.agreed_price_in);
                    r.agreed_price_out = Some(e.agreed_price_out);
                }
            }
            Event::RequestSettled(e) => {
                if let Some(r) = self.live_request(&e.request.to_string(), "assigned") {
                    r.status = "settled".into();
                    r.in_tokens = Some(e.in_tokens);
                    r.out_tokens = Some(e.out_tokens);
                    r.measured_tps = Some(e.measured_tps);
                    r.due = Some(e.due);
                    r.fee = Some(e.fee);
                    r.refund = Some(e.refund);
                    r.closed_at = tx.block_time;
                    self.live.remove(&e.request.to_string());
                }
            }
            Event::RequestCancelled(e) => {
                if let Some(r) = self.live_request(&e.request.to_string(), "open") {
                    r.status = "cancelled".into();
                    r.refund = Some(e.refund);
                    r.closed_at = tx.block_time;
                    self.live.remove(&e.request.to_string());
                }
            }
        }
    }

    fn live_request(&mut self, request: &str, status: &str) -> Option<&mut RequestRecord> {
        let i = *self.live.get(request)?;
        Some(&mut self.requests[i]).filter(|r| r.status == status)
    }

    /// Requests matching the filters, newest first.
    pub fn requests(&self, client: Option<&str>, worker: Option<&str>) -> Vec<RequestRecord> {
        self.requests
            .iter()
            .rev()
            .filter(|r| client.is_none_or(|c| r.client == c))
            .filter(|r| worker.is_none_or(|w| r.worker.as_deref() == Some(w)))
            .cloned()
            .collect()
    }

    /// Totals over the settled jobs of the worker owned by `owner`; `None` if it never registered.
    pub fn earnings(&self, owner: &str) -> Option<Earnings> {
        let w = self.workers.values().find(|w| w.owner == owner)?;
        let settled: Vec<&RequestRecord> = self
            .requests
            .iter()
            .filter(|r| r.status == "settled" && r.worker.as_deref() == Some(&w.worker))
            .collect();
        let sum = |f: fn(&RequestRecord) -> u64| settled.iter().map(|r| f(r)).sum::<u64>();
        Some(Earnings {
            owner: w.owner.clone(),
            worker: w.worker.clone(),
            model: w.model.clone(),
            tok_per_s: w.tok_per_s,
            price_in: w.price_in,
            price_out: w.price_out,
            jobs: settled.len() as u64,
            paid_jobs: settled.iter().filter(|r| r.due.unwrap_or(0) > 0).count() as u64,
            in_tokens: sum(|r| r.in_tokens.unwrap_or(0)),
            out_tokens: sum(|r| r.out_tokens.unwrap_or(0)),
            earned: sum(|r| r.due.unwrap_or(0) - r.fee.unwrap_or(0)),
            last_job: settled.last().map(|r| (*r).clone()),
        })
    }

    /// Everything rebuilt from the chain, in a canonical order, to compare two indexes.
    pub fn dump(&self) -> serde_json::Value {
        let mut workers: Vec<&WorkerRow> = self.workers.values().collect();
        workers.sort_by(|a, b| a.worker.cmp(&b.worker));
        serde_json::json!({ "events": self.events, "workers": workers, "requests": self.requests })
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.events.len(), self.requests.len())
    }
}

#[cfg(test)]
mod tests {
    use common::client::types::Model;
    use solana_address::Address;

    use super::*;
    use crate::events::*;

    fn addr(n: u8) -> Address {
        Address::from([n; 32])
    }

    fn tx(sig: &str, slot: u64, events: Vec<Event>) -> IndexedTx {
        IndexedTx {
            signature: sig.into(),
            slot,
            block_time: Some(slot as i64),
            events,
        }
    }

    /// A worker, then the same nonce used twice: settled first, cancelled after.
    fn history() -> Vec<IndexedTx> {
        let (worker, owner, request, client) = (addr(1), addr(2), addr(3), addr(4));
        let created = |nonce| {
            Event::RequestCreated(RequestCreated {
                request,
                client,
                nonce,
                model: Model::Qwen38,
                min_tps: 30,
                max_price_in: 150,
                max_price_out: 500,
                budget: 1_000_000,
                prompt_hash: [0; 32],
                request_fee: 10_000,
            })
        };
        vec![
            tx(
                "s1",
                1,
                vec![Event::WorkerRegistered(WorkerRegistered {
                    worker,
                    owner,
                    model: Model::Qwen38,
                    tok_per_s: 40,
                    price_in: 100,
                    price_out: 400,
                })],
            ),
            tx("s2", 2, vec![created(7)]),
            tx(
                "s3",
                3,
                vec![Event::WorkerAssigned(WorkerAssigned {
                    request,
                    worker,
                    agreed_price_in: 100,
                    agreed_price_out: 400,
                })],
            ),
            tx(
                "s4",
                4,
                vec![Event::RequestSettled(RequestSettled {
                    request,
                    client,
                    worker,
                    in_tokens: 3,
                    out_tokens: 70,
                    measured_tps: 38,
                    due: 28_300,
                    fee: 566,
                    refund: 971_700,
                })],
            ),
            tx("s5", 5, vec![created(7)]),
            tx(
                "s6",
                6,
                vec![Event::RequestCancelled(RequestCancelled {
                    request,
                    client,
                    refund: 1_000_000,
                })],
            ),
        ]
    }

    #[test]
    fn projects_lifecycle_and_nonce_reuse() {
        let mut index = Index::default();
        history().iter().for_each(|t| index.apply(t));
        let requests = index.requests(Some(&addr(4).to_string()), None);
        let statuses: Vec<_> = requests
            .iter()
            .map(|r| (r.id.as_str(), r.status.as_str()))
            .collect();
        assert_eq!(statuses, [("s5", "cancelled"), ("s2", "settled")]);
        assert_eq!(requests[1].due, Some(28_300));
        assert_eq!(index.last_signature(), Some("s6"));

        let earnings = index.earnings(&addr(2).to_string()).unwrap();
        assert_eq!(
            (
                earnings.jobs,
                earnings.paid_jobs,
                earnings.out_tokens,
                earnings.earned
            ),
            (1, 1, 70, 27_734)
        );
        assert!(index.earnings(&addr(9).to_string()).is_none());
    }

    /// Polling in batches with overlaps (or replaying everything) gives the same index as one
    /// pass over the history.
    #[test]
    fn replay_with_overlap_is_identical() {
        let txs = history();
        let mut once = Index::default();
        txs.iter().for_each(|t| once.apply(t));

        let mut batched = Index::default();
        txs[..3].iter().for_each(|t| batched.apply(t));
        txs[1..].iter().for_each(|t| batched.apply(t));
        txs.iter().for_each(|t| batched.apply(t));
        assert_eq!(batched.dump(), once.dump());
    }
}
