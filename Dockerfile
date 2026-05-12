FROM rust:1.87-slim-bookworm AS builder

ARG RUST_TARGET_CPU=haswell
ENV RUSTFLAGS="-C target-cpu=${RUST_TARGET_CPU}"

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /lb
COPY Cargo.toml ./
COPY src/ ./src/

RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /lb/target/release/rinhaLb /usr/local/bin/lb
ENTRYPOINT ["/usr/local/bin/lb"]
