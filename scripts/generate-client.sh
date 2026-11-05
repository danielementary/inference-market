#!/usr/bin/env bash
# Regenerate the Rust client (clients/rust/src/generated) from onchain/target/idl/inference_escrow.json.
# Run `anchor build` in onchain/ first if the program changed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CODAMA_DIR="$ROOT/clients/codama"

if [ ! -d "$CODAMA_DIR/node_modules" ]; then
  npm ci --prefix "$CODAMA_DIR"
fi
node "$CODAMA_DIR/generate.mjs"

if [ "${1:-}" = "--check" ]; then
  cargo build --manifest-path "$ROOT/clients/rust/Cargo.toml"
fi
