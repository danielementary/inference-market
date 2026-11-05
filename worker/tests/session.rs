//! Drives a worker session against a fake coordinator (a WebSocket server inside the test).

use std::time::{Duration, Instant};

use common::{
    client::types::Model,
    protocol::{verify_challenge, CoordinatorMsg, WorkerMsg},
    Address, Keypair, Signer,
};
use futures_util::{SinkExt, StreamExt};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message, WebSocketStream};
use worker::{
    fault::CRASH_AFTER,
    responses,
    session::{self, Offer},
};

const TPS: u32 = 100;
const OFFER: Offer = Offer {
    model: Model::Qwen38,
    tok_per_s: TPS,
};

struct FakeCoordinator {
    ws: WebSocketStream<tokio::net::TcpStream>,
    worker: JoinHandle<anyhow::Result<()>>,
}

impl FakeCoordinator {
    /// Starts a worker and completes the handshake, checking the signature.
    async fn start(faulty: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let owner = Keypair::new();
        let owner_key = owner.pubkey();
        let worker = tokio::spawn(async move { session::run(&url, &owner, OFFER, faulty).await });

        let (tcp, _) = listener.accept().await.unwrap();
        let mut this = Self {
            ws: accept_async(tcp).await.unwrap(),
            worker,
        };
        let nonce = [42; 32];
        this.send(CoordinatorMsg::Challenge { nonce }).await;
        let WorkerMsg::Auth { owner, signature } = this.recv().await.unwrap() else {
            panic!("expected auth");
        };
        assert_eq!(owner, owner_key.to_string());
        assert!(verify_challenge(
            &owner.parse::<Address>().unwrap(),
            &nonce,
            &signature
        ));
        this.send(CoordinatorMsg::Ready).await;
        this
    }

    async fn send(&mut self, msg: CoordinatorMsg) {
        let text = serde_json::to_string(&msg).unwrap();
        self.ws.send(Message::text(text)).await.unwrap();
    }

    /// Next worker message, or `None` once the connection is gone.
    async fn recv(&mut self) -> Option<WorkerMsg> {
        while let Some(Ok(frame)) = self.ws.next().await {
            if let Message::Text(text) = frame {
                return Some(serde_json::from_str(&text).unwrap());
            }
        }
        None
    }

    async fn job(&mut self, request: &str, max_out_tokens: u64) {
        let job = CoordinatorMsg::Job {
            request: request.into(),
            prompt: "Explain rent".into(),
            max_out_tokens,
        };
        self.send(job).await;
    }

    /// Collects chunks until `Done` (true) or a disconnect (false), with the elapsed time.
    async fn collect(&mut self, request: &str) -> (Vec<String>, bool, Duration) {
        let start = Instant::now();
        let mut tokens = Vec::new();
        loop {
            match self.recv().await {
                Some(WorkerMsg::Chunk {
                    request: r,
                    seq,
                    token,
                }) => {
                    assert_eq!(r, request);
                    assert_eq!(seq, tokens.len() as u64, "chunks must be in order");
                    tokens.push(token);
                }
                Some(WorkerMsg::Done { request: r }) => {
                    assert_eq!(r, request);
                    return (tokens, true, start.elapsed());
                }
                Some(other) => panic!("unexpected {other:?}"),
                None => return (tokens, false, start.elapsed()),
            }
        }
    }
}

fn expected(n: usize) -> Vec<String> {
    responses::tokens(Model::Qwen38)
        .into_iter()
        .take(n)
        .map(String::from)
        .collect()
}

#[tokio::test]
async fn streams_up_to_max_tokens_at_advertised_rate() {
    let mut c = FakeCoordinator::start(false).await;
    c.job("req-1", 10).await;
    let (tokens, done, elapsed) = c.collect("req-1").await;

    assert!(done);
    assert_eq!(tokens, expected(10));
    // 10 tokens at 100 tok/s: about 100 ms.
    assert!(
        elapsed >= Duration::from_millis(95),
        "too fast: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(600),
        "too slow: {elapsed:?}"
    );

    // The session stays up for the next job.
    c.job("req-2", 3).await;
    assert_eq!(c.collect("req-2").await.0, expected(3));
}

#[tokio::test]
async fn stops_at_end_of_response() {
    let mut c = FakeCoordinator::start(false).await;
    let all = responses::tokens(Model::Qwen38).len();
    c.job("req", all as u64 + 100).await;
    let (tokens, done, _) = c.collect("req").await;
    assert!(done);
    assert_eq!(tokens.len(), all);
}

#[tokio::test]
async fn faulty_is_slow_then_silent_then_crashes() {
    let mut c = FakeCoordinator::start(true).await;

    // 1. Slow: half the rate, so 10 tokens take about 200 ms.
    c.job("slow", 10).await;
    let (tokens, done, elapsed) = c.collect("slow").await;
    assert!(done);
    assert_eq!(tokens.len(), 10);
    assert!(
        elapsed >= Duration::from_millis(190),
        "not slow: {elapsed:?}"
    );

    // 2. No start: nothing comes back.
    c.job("silent", 10).await;
    assert!(
        timeout(Duration::from_millis(300), c.recv()).await.is_err(),
        "worker should stay silent"
    );

    // 3. Crash after a few tokens: no `Done`, the connection drops.
    c.job("crash", 10).await;
    let (tokens, done, _) = c.collect("crash").await;
    assert!(!done);
    assert_eq!(tokens, expected(CRASH_AFTER as usize));
    let result = c.worker.await.unwrap();
    assert!(result.unwrap_err().to_string().contains("simulated crash"));
}

#[tokio::test]
async fn refused_handshake_ends_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let worker =
        tokio::spawn(async move { session::run(&url, &Keypair::new(), OFFER, false).await });
    let (tcp, _) = listener.accept().await.unwrap();
    let mut ws = accept_async(tcp).await.unwrap();
    let challenge = serde_json::to_string(&CoordinatorMsg::Challenge { nonce: [1; 32] }).unwrap();
    ws.send(Message::text(challenge)).await.unwrap();
    ws.next().await; // auth
    ws.close(None).await.unwrap();
    assert!(worker.await.unwrap().is_err());
}
