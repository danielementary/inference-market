use std::path::PathBuf;

use anyhow::{anyhow, Result};
use solana_keypair::{read_keypair_file, Keypair};

/// Directory holding the demo keypairs: `$KEYS_DIR`, or the repo's `keys/`.
pub fn keys_dir() -> PathBuf {
    std::env::var_os("KEYS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../keys")))
}

/// Loads `<keys_dir>/<name>.json`, e.g. `load("coordinator")`.
pub fn load(name: &str) -> Result<Keypair> {
    let path = keys_dir().join(format!("{name}.json"));
    read_keypair_file(&path).map_err(|e| anyhow!("reading {}: {e}", path.display()))
}
