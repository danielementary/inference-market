#!/usr/bin/env bash
# Start a local validator with the program preloaded (upgrade authority: keys/admin.json,
# required by init_config), and fund the demo keys.
set -euo pipefail
cd "$(dirname "$0")/.."

RPC=http://127.0.0.1:8899
PROGRAM_ID=$(solana-keygen pubkey onchain/target/deploy/inference_escrow-keypair.json)

solana-test-validator --reset --quiet \
  --upgradeable-program "$PROGRAM_ID" onchain/target/deploy/inference_escrow.so keys/admin.json &
VALIDATOR_PID=$!
trap 'kill $VALIDATOR_PID' EXIT

until solana -u "$RPC" cluster-version >/dev/null 2>&1; do sleep 0.5; done
for k in keys/*.json; do
  solana -u "$RPC" airdrop 100 "$k" >/dev/null
done
echo "localnet ready at $RPC (program $PROGRAM_ID)"
wait $VALIDATOR_PID
