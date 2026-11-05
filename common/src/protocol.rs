//! Coordinator ↔ worker messages, sent as JSON text frames over a WebSocket opened by the worker.
//!
//! Handshake: `Challenge` → `Auth` (owner signs the nonce) → `Ready`. Then the coordinator sends
//! one `Job` at a time; the worker streams `Chunk`s and ends with `Done` or `Failed`. The
//! coordinator counts chunks and times them itself: nothing the worker sends is trusted for
//! payment.

use serde::{Deserialize, Serialize};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signature::Signature;
use solana_signer::Signer;

/// Addresses (owner, request) travel as base58 strings.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoordinatorMsg {
    Challenge {
        nonce: [u8; 32],
    },
    Ready,
    Job {
        request: String,
        prompt: String,
        max_out_tokens: u64,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerMsg {
    Auth {
        owner: String,
        signature: String,
    },
    Chunk {
        request: String,
        seq: u64,
        token: String,
    },
    Done {
        request: String,
    },
    Failed {
        request: String,
        reason: String,
    },
}

/// Body of `POST /requests` (client → coordinator). The coordinator recomputes
/// `sha256(salt || prompt)` and checks it against the on-chain `prompt_hash`, so only the
/// client who created the request can submit its prompt.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PromptRequest {
    pub client: String,
    pub nonce: u64,
    pub prompt: String,
    pub salt: [u8; 32],
}

/// Server-sent events answering `POST /requests`, in order: `Queued`, `Assigned`, `Chunk`s,
/// then `Settled` or `Error`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    Queued,
    Assigned {
        worker: String,
        signature: String,
    },
    Chunk {
        token: String,
    },
    Settled {
        signature: String,
        in_tokens: u64,
        out_tokens: u64,
        measured_tps: u32,
        due: u64,
        fee: u64,
        refund: u64,
    },
    Error {
        reason: String,
    },
}

/// Domain-separated message signed during the handshake, so the signature can't be replayed
/// as anything else.
fn challenge_message(nonce: &[u8; 32]) -> Vec<u8> {
    [b"inference-escrow worker auth:".as_slice(), nonce].concat()
}

pub fn sign_challenge(owner: &Keypair, nonce: &[u8; 32]) -> String {
    owner.sign_message(&challenge_message(nonce)).to_string()
}

pub fn verify_challenge(owner: &Address, nonce: &[u8; 32], signature: &str) -> bool {
    signature
        .parse::<Signature>()
        .is_ok_and(|sig| sig.verify(owner.as_ref(), &challenge_message(nonce)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_signature_round_trip() {
        let owner = Keypair::new();
        let nonce = [9; 32];
        let sig = sign_challenge(&owner, &nonce);
        assert!(verify_challenge(&owner.pubkey(), &nonce, &sig));
        assert!(!verify_challenge(&owner.pubkey(), &[8; 32], &sig));
        assert!(!verify_challenge(&Keypair::new().pubkey(), &nonce, &sig));
        assert!(!verify_challenge(
            &owner.pubkey(),
            &nonce,
            "not-a-signature"
        ));
    }

    #[test]
    fn messages_are_tagged_json() {
        let msg = WorkerMsg::Done {
            request: "abc".into(),
        };
        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            r#"{"type":"done","request":"abc"}"#
        );
    }
}
