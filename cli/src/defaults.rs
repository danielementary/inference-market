//! Last max prices used, remembered across runs in `~/.config/infer/defaults.json`.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prices {
    pub max_price_in: u64,
    pub max_price_out: u64,
}

fn path() -> Result<PathBuf> {
    let config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME not set")?).join(".config"),
    };
    Ok(config.join("infer").join("defaults.json"))
}

/// Saved prices, or `None` if nothing was saved (or the file is unreadable).
pub fn load() -> Option<Prices> {
    serde_json::from_slice(&fs::read(path().ok()?).ok()?).ok()
}

pub fn save(prices: Prices) -> Result<()> {
    let path = path()?;
    fs::create_dir_all(path.parent().expect("has a parent"))?;
    fs::write(&path, serde_json::to_vec_pretty(&prices)?)
        .with_context(|| format!("writing {}", path.display()))
}

pub fn reset() -> Result<()> {
    match fs::remove_file(path()?) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}
