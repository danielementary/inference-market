# syntax=docker/dockerfile:1
# Off-chain services and CLI. The validator runs on the host (scripts/localnet.sh).

FROM rust:1.99-bookworm AS builder
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bins --examples -p cli -p coordinator -p indexer -p worker \
    && mkdir /out \
    && cp target/release/coordinator target/release/indexer target/release/worker \
          target/release/infer target/release/examples/e2e /out/

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /out/ /usr/local/bin/
ENV KEYS_DIR=/keys \
    RPC_URL=http://host.docker.internal:8899
