# inference-market

Distributed inference for open-weight models, paid in SOL. Clients pick a model, a minimum
throughput and a budget; a coordinator matches them with a worker (a simulated Mac), relays the
streamed answer, and settles on-chain. The budget sits in escrow in a program-owned account until
settlement, which pays the worker for the tokens produced and refunds the rest to the client.

## The idea

People with idle Macs rent out their compute to run open-weight models, and get paid in SOL for
the tokens they produce. A client asks for a model and a minimum throughput ("Qwen 3.8, at least
30 tokens/s") with a maximum budget, accepts the prices and locks the funds. A coordinator picks a
compatible machine, forwards the prompt and relays the answer as a stream, then attests what was
consumed so that the program pays the worker and refunds the rest to the client.

Prompts and answers stay off-chain. The chain holds what no party should be able to bypass: the
escrowed funds, the accepted terms, the state transitions and the payment rule.

## Approach

I first built the whole system end to end, each time with the simplest solution that works: the
program, the coordinator, two simulated workers, an indexer, a CLI, tests and a Docker demo. I
then spent most of the time questioning it: which guarantees really hold, how a malicious client
or worker could abuse it, and what still relies on trusting the coordinator. The machines, models
and throughputs are simulated: each worker streams a canned answer at its advertised rate, and one
of them misbehaves on purpose.

## How it works

```
                    ┌─────────────── transactions ───────────────┐
                    │                                            ▼
Client (infer CLI) ─┼─ HTTP (prompt) / SSE (stream) ─▶ Coordinator ◀── WebSocket ── Workers (×2)
                    │                                   │
                    ▼                                   ▼
            Solana program inference_escrow (escrow, offers, lifecycle, settlement)
                    │ events (emit_cpi)
                    ▼
                Indexer ── REST ──▶ CLI (history, earnings)
```

### Accounts

| Account | PDA seeds | Size | Holds |
|---|---|---|---|
| `Config` | `["config"]` | 51 B | coordinator identity, `fee_bps`, `request_fee`; created once by the upgrade authority, immutable |
| `Worker` | `["worker", owner]` | 63 B | one offer: model, advertised tokens/s, price per input and output token, `active` |
| `Request` | `["request", client, nonce]` | 159 B | the accepted terms, `prompt_hash`, status, assigned worker and frozen prices; **the account is the escrow** |

### Lifecycle of a request

```
create_request (client)       assign_worker (coordinator)        settle (coordinator)
        │                               │                                 │
        ▼                               ▼                                 ▼
     [Open] ──────────────────────▶ [Assigned] ─────────────────────▶ completed (account closed)
        │
        └── cancel_request (client) ──▶ cancelled (account closed)
```

1. **Quote.** The CLI reads the offers on-chain, proposes max prices and a budget, and refuses a
   request that no offer can serve.
2. **`create_request`** (client): stores the terms and `prompt_hash = sha256(salt ‖ prompt)`,
   deposits the budget into the `Request` account and pays `request_fee` to the coordinator.
3. **Prompt.** The client sends the prompt and its salt to the coordinator, which checks the
   request on-chain and the hash, then queues it.
4. **`assign_worker`** (coordinator): picks the cheapest free compatible worker; the program
   checks compatibility (model, throughput, prices, `active`) and **freezes the worker's prices**.
5. **Stream.** The worker streams the answer token by token over its WebSocket; the coordinator
   relays it to the client, counts and times what it receives, and stops at the last token the
   budget pays for.
6. **`settle`** (coordinator): declares the measured usage; the program computes the amount due,
   pays the worker and the commission, refunds the rest and the rent to the client, and closes
   the account.

The client can **cancel** while the request is `Open` (budget and rent back, `request_fee` kept).
A job that fails (silent worker, too slow) is settled with zero usage, which refunds the whole
budget. Every instruction emits an event (`emit_cpi!`); the indexer replays them to rebuild the
history.

### Payment rule

```
cost = in_tokens × agreed_price_in + out_tokens × agreed_price_out     (checked arithmetic)
due  = 0                  if measured_tps < min_tps
     = min(cost, budget)  otherwise
fee  = due × fee_bps / 10 000
```

The worker's owner receives `due − fee`, the coordinator `fee` (plus `request_fee` at creation),
and the client gets back `budget − due` plus the rent. The coordinator attests **measurements**
(`in_tokens`, `out_tokens`, `measured_tps`), never an amount: the amount is always computed by
the program. `request_fee` (10,000 lamports) and `fee_bps` (200, i.e. 2%) are illustrative
defaults set by `infer admin init`.

### Trust model

The coordinator is trusted; clients and workers are not. Whatever the coordinator does, the
program guarantees that:

- escrowed funds only leave through `cancel_request` (client, `Open`) or `settle` (coordinator,
  `Assigned`);
- a payment never exceeds the budget nor the frozen prices times the attested tokens;
- only the assigned worker's owner is paid, and the refund always goes to the request's client;
- a request is settled or cancelled exactly once, and lamports are conserved;
- only the upgrade authority can initialize the config, once.

Off-chain, the coordinator checks that the executed prompt is the committed one, meters usage
itself instead of trusting the worker, and settles at zero any request left `Assigned` by a
restart. It still decides which worker serves which request, what usage it declares, and whether
an assigned request ever gets settled.

### Storage and costs

A `Request` exists only while the request is in progress: `settle` and `cancel_request` close it
and return its rent (~0.0020 SOL) to the client. On-chain state is therefore one `Config`, one
`Worker` per registered worker and one `Request` per request in progress, whatever the size of the
history, which lives in the transactions as events. Every instruction stays between ~5,000 and
~20,000 compute units, and a full request costs three transactions.

## Limits and next steps

- **Coordinator liveness:** an `Assigned` request can only be closed by the coordinator; an
  on-chain deadline after assignment would let the client recover its funds on its own.
- **Verifiable usage:** a worker signature over what it delivered, checked in `settle` through the
  ed25519 precompile, and a `max_out_tokens` bound chosen by the client.
- **Unserved requests:** a request that no worker picks up costs the client its `request_fee`;
  an expiry with a delay that doubles for repeated expired requests would refund it without
  opening a spam vector.
- **Incentives:** paying `request_fee` at assignment rather than at creation, an optional priority
  fee, and a worker stake slashed when nothing is delivered.
- **Worker accounts:** `update_worker`, `pause_worker`, `close_worker`, and separate cold and hot
  keys.
- **Catalogue:** an on-chain model registry instead of an enum, and several models per worker.
- **Indexer:** a persistent database with a resume point instead of replaying everything on start.
- **Latency:** two confirmations before the first token; a combined create-and-assign transaction
  would need only one.

## Layout

```
onchain/                 Anchor project: the inference_escrow program and its LiteSVM tests
clients/codama/          Codama generator: IDL → Rust client
clients/rust/            generated Rust client (inference-escrow-client)
common/                  shared off-chain code: RPC, instruction builders, prompt hashing, protocols
coordinator/             matching, stream relay, metering, assign/settle (axum, :8080)
worker/                  simulated Mac (--faulty cycles slow / no start / crash)
indexer/                 rebuilds history from transactions on every start, REST API (:8090)
cli/                     `infer`, the client/worker CLI, and the e2e scenarios
scripts/                 localnet.sh (host validator), generate-client.sh
Dockerfile, docker-compose.yml   off-chain stack against the host validator
```

The root `Cargo.toml` is the off-chain workspace; `onchain/` has its own, so the program build
never sees off-chain dependencies.

## Prerequisites

| Tool | Version used |
|---|---|
| Rust | 1.99.1 (pinned by `rust-toolchain.toml`) |
| Solana CLI (Agave) | 4.1.2 |
| Anchor | 1.2.1 |
| Docker + Compose | 29 / v5 (for the demo) |
| Node.js | 24 (only to regenerate the client) |

## Setup (once, after cloning)

The demo keypairs and the program keypair are not committed.

```bash
mkdir -p keys
for k in admin coordinator client-a worker-1 worker-2; do solana-keygen new --no-bip39-passphrase --silent -o keys/$k.json; done
```

Build the program. On a fresh clone, `anchor keys sync` writes the new program ID into the
program and `Anchor.toml`; then regenerate the client, which embeds that ID:

```bash
cd onchain && anchor keys sync && anchor build && cd ..
scripts/generate-client.sh --check
```

## Demo (Docker)

All off-chain code runs in Docker; the validator runs on the host.

1. Start the validator in its own terminal (fresh ledger, program preloaded with `keys/admin.json`
   as upgrade authority, demo keys funded). Wait for `localnet ready`:

   ```bash
   scripts/localnet.sh
   ```

2. Start the stack: `init` (program config, idempotent), the coordinator, two workers and the
   indexer:

   ```bash
   docker compose up -d --build
   ```

   `worker-1` serves `qwen38` (40 tok/s, honest); `worker-2` serves `llama4` (30 tok/s,
   `--faulty`: each job is slow, then silent, then crashes after 5 tokens, in a loop).

3. Play with the client:

   ```bash
   docker compose run --rm cli offers
   docker compose run --rm cli request --model qwen38 --min-tps 30 "Explain Solana rent"
   docker compose run --rm cli request --model llama4 --min-tps 25 "Try the faulty worker"
   docker compose run --rm cli history
   docker compose run --rm cli earnings --worker worker-1
   ```

4. Run the end-to-end scenarios (honest payout, the three faults, cancellation while queued):

   ```bash
   docker compose run --rm e2e
   ```

5. Watch what happens:

   ```bash
   docker compose logs -f coordinator
   docker compose logs -f --no-log-prefix indexer
   ```

6. Stop: `docker compose down`, then Ctrl-C in the validator terminal. `scripts/localnet.sh`
   resets the ledger on each start; run `docker compose up -d` again afterwards so `init`
   recreates the config.

Ports 8080 (coordinator) and 8090 (indexer) are published, so the CLI also works from the host:
`cargo run -p cli -- <command>`.

## Running without Docker

With `scripts/localnet.sh` running, from the repository root, each in its own terminal:

```bash
cargo run -p cli -- admin init
cargo run -p coordinator
cargo run -p worker -- --key worker-1 --model qwen38 --tps 40 --price-in 100 --price-out 400
cargo run -p worker -- --key worker-2 --model llama4 --tps 30 --price-in 80 --price-out 300 --faulty
cargo run -p indexer
cargo run -p cli -- request --model qwen38 --min-tps 30 "Explain Solana rent"
```

## CLI (`infer`)

| Command | Does |
|---|---|
| `offers` | lists the workers' on-chain offers |
| `request --model <m> --min-tps <n> [--max-out N] [--max-price-in N --max-price-out N] [--budget N] [--dry-run] [--yes] "<prompt>"` | quotes, creates the request (confirmation on a terminal), streams the answer, prints the settlement |
| `cancel <nonce>` | cancels an open request (budget and rent back, request fee kept) |
| `requests` | your requests still on-chain |
| `history` | your past requests, from the indexer |
| `earnings --worker <key>` | a worker's earnings, from the indexer |
| `prices [--reset]` | shows or forgets the remembered max prices |
| `admin init [--fee-bps N] [--request-fee N]` | initializes the program config (upgrade authority) |

Global options: `--key <name>` (keypair in `keys/`, default `client-a`), `--coordinator`,
`--indexer`; `RPC_URL` (default `http://127.0.0.1:8899`).

## Tests

```bash
cd onchain && anchor test          # program: LiteSVM integration tests + payout unit tests
cargo test --workspace             # off-chain unit and integration tests (from the root)
docker compose run --rm e2e        # end-to-end scenarios against the running stack
```

After changing the program, rebuild it (`anchor build` in `onchain/`) and regenerate the client
(`scripts/generate-client.sh --check`).

## AI assistance

Built with the help of Claude (Claude Opus 5.5, via Claude Code), used to explore design options,
implement each step from a plan I approved and reviewed, and stress-test the design for attacks.

## License

Copyright 2026 Daniel Filipe Nunes Silva. Licensed under the [Apache License, Version 2.0](LICENSE).
