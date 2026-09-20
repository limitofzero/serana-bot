# syntax=docker/dockerfile:1

# Dependency compilation is cached separately from application code with cargo-chef:
# a source-only change then rebuilds the workspace crates, not the dependency tree.
FROM rust:1.97-slim AS chef
WORKDIR /app
RUN cargo install cargo-chef --locked --version ^0.1

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked --bin serana --bin serana-tg

# rustls means no OpenSSL at runtime; only the trust store is needed.
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini \
 && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 serana
COPY --from=builder /app/target/release/serana /usr/local/bin/serana
COPY --from=builder /app/target/release/serana-tg /usr/local/bin/serana-tg

# Conversations, memory and any future index live here; mount it to persist them.
ENV SERANA_DATA_DIR=/data
RUN mkdir -p /data && chown serana:serana /data
VOLUME ["/data"]

USER serana
WORKDIR /data

# tini reaps zombies and forwards SIGTERM, so the agent can close an open tool round
# and flush its session before the container dies.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["serana-tg"]
