use sha2::{Digest, Sha256};

/// Random salt the client keeps off-chain alongside the prompt.
pub fn new_salt() -> [u8; 32] {
    rand::random()
}

/// Commitment stored on-chain in `Request::prompt_hash`: `sha256(salt || prompt)`.
pub fn prompt_hash(salt: &[u8; 32], prompt: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(prompt.as_bytes());
    hasher.finalize().into()
}

/// Simulated tokenizer: one token per whitespace-separated word. Shared by the client
/// (budget estimate) and the coordinator (attested `in_tokens`) so both count the same way.
pub fn count_tokens(text: &str) -> u64 {
    text.split_whitespace().count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_and_salted() {
        let salt = [1; 32];
        assert_eq!(prompt_hash(&salt, "hello"), prompt_hash(&salt, "hello"));
        assert_ne!(prompt_hash(&salt, "hello"), prompt_hash(&[2; 32], "hello"));
        assert_ne!(prompt_hash(&salt, "hello"), prompt_hash(&salt, "hello!"));
    }

    #[test]
    fn salts_differ() {
        assert_ne!(new_salt(), new_salt());
    }

    #[test]
    fn counts_words() {
        assert_eq!(count_tokens(""), 0);
        assert_eq!(count_tokens("  Explain   Solana rent\nbriefly "), 4);
    }
}
