//! Online workers and the FIFO queue of prompts waiting for one.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use common::{
    chain::Chain,
    client::accounts::{Request, Worker},
    protocol::{CoordinatorMsg, StreamEvent, WorkerMsg},
    Address, Keypair,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::{job, matching};

pub struct App {
    pub chain: Chain,
    pub key: Keypair,
    pub fee_bps: u16,
    pub timeouts: Timeouts,
    inner: Mutex<Inner>,
}

#[derive(Clone, Copy)]
pub struct Timeouts {
    /// Wait for the first token after sending a job (catches workers that never start).
    pub first_chunk: Duration,
    /// Wait between two tokens (catches stalled workers).
    pub chunk: Duration,
}

#[derive(Default)]
struct Inner {
    /// Authenticated, connected workers by owner.
    workers: HashMap<Address, Slot>,
    /// Prompts waiting for a compatible free worker, in arrival order.
    queue: VecDeque<Pending>,
    /// Requests queued or running, to reject a second submission.
    active: HashSet<Address>,
    next_conn: u64,
}

struct Slot {
    /// Distinguishes reconnections of the same owner.
    conn: u64,
    /// On-chain offer read at connection time.
    offer: Worker,
    /// Messages to the worker's socket.
    tx: UnboundedSender<CoordinatorMsg>,
    busy: bool,
    /// Sink for the current job's messages.
    job: Option<UnboundedSender<WorkerMsg>>,
}

/// A prompt checked against its on-chain request and waiting for a worker.
pub struct Pending {
    pub request: Address,
    pub client: Address,
    pub nonce: u64,
    pub terms: Request,
    pub prompt: String,
    pub in_tokens: u64,
    pub events: mpsc::Sender<StreamEvent>,
}

/// A pending prompt matched with a worker.
pub struct Job {
    pub pending: Pending,
    pub owner: Address,
    pub conn: u64,
    pub offer: Worker,
    pub worker_tx: UnboundedSender<CoordinatorMsg>,
    pub messages: UnboundedReceiver<WorkerMsg>,
}

impl App {
    pub fn new(chain: Chain, key: Keypair, fee_bps: u16, timeouts: Timeouts) -> Self {
        Self {
            chain,
            key,
            fee_bps,
            timeouts,
            inner: Mutex::default(),
        }
    }

    /// Registers a connected worker. `None` if this owner is already connected.
    pub fn add_worker(
        &self,
        owner: Address,
        offer: Worker,
        tx: UnboundedSender<CoordinatorMsg>,
    ) -> Option<u64> {
        let mut inner = self.inner.lock().unwrap();
        if inner.workers.contains_key(&owner) {
            return None;
        }
        inner.next_conn += 1;
        let conn = inner.next_conn;
        inner.workers.insert(
            owner,
            Slot {
                conn,
                offer,
                tx,
                busy: false,
                job: None,
            },
        );
        Some(conn)
    }

    /// Forgets a disconnected worker. Dropping its job sink ends the running job, if any.
    pub fn remove_worker(&self, owner: &Address, conn: u64) {
        let mut inner = self.inner.lock().unwrap();
        if inner.workers.get(owner).is_some_and(|s| s.conn == conn) {
            inner.workers.remove(owner);
        }
    }

    /// Forwards a worker message to its current job; dropped when idle.
    pub fn route(&self, owner: &Address, conn: u64, msg: WorkerMsg) {
        let inner = self.inner.lock().unwrap();
        if let Some(job) = inner
            .workers
            .get(owner)
            .filter(|s| s.conn == conn)
            .and_then(|s| s.job.as_ref())
        {
            let _ = job.send(msg);
        }
    }

    /// Queues a prompt. `false` if this request is already queued or running.
    pub fn enqueue(&self, pending: Pending) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if !inner.active.insert(pending.request) {
            return false;
        }
        inner.queue.push_back(pending);
        true
    }

    /// Frees the worker and the request once a job is over.
    pub fn finish(&self, request: &Address, owner: &Address, conn: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.active.remove(request);
        if let Some(slot) = inner.workers.get_mut(owner).filter(|s| s.conn == conn) {
            slot.busy = false;
            slot.job = None;
        }
    }

    /// Starts every queued prompt that has a compatible free worker, oldest first, each on the
    /// cheapest such worker.
    pub fn dispatch(self: &Arc<Self>) {
        let mut jobs = Vec::new();
        {
            let mut inner = self.inner.lock().unwrap();
            let mut i = 0;
            while i < inner.queue.len() {
                let pending = &inner.queue[i];
                let free = inner
                    .workers
                    .iter()
                    .filter(|(_, s)| {
                        !s.busy && matching::compatible(&s.offer, &pending.terms, pending.in_tokens)
                    })
                    .map(|(owner, s)| (*owner, &s.offer));
                let Some(owner) = matching::cheapest(free, pending.in_tokens) else {
                    i += 1;
                    continue;
                };
                let pending = inner.queue.remove(i).expect("index in bounds");
                let (job_tx, messages) = mpsc::unbounded_channel();
                let slot = inner.workers.get_mut(&owner).expect("picked from workers");
                slot.busy = true;
                slot.job = Some(job_tx);
                jobs.push(Job {
                    pending,
                    owner,
                    conn: slot.conn,
                    offer: slot.offer.clone(),
                    worker_tx: slot.tx.clone(),
                    messages,
                });
            }
        }
        for job in jobs {
            tokio::spawn(job::run(self.clone(), job));
        }
    }
}
