FROM rust:1.87-slim-bookworm AS builder

ARG RUST_TARGET_CPU=haswell
ENV RUSTFLAGS="-C target-cpu=${RUST_TARGET_CPU} -C panic=abort -C link-arg=-Wl,--gc-sections"

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /lb
COPY Cargo.toml ./
COPY src/ ./src/

RUN cargo build --release

FROM gcr.io/distroless/cc
COPY --from=builder /lb/target/release/rinhaLb /usr/local/bin/lb
ENTRYPOINT ["/usr/local/bin/lb"]
