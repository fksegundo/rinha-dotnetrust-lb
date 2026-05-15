# rinha-dotnetrust-lb

[![Build image](https://github.com/fksegundo/rinha-dotnetrust-lb/actions/workflows/publish-image.yml/badge.svg)](https://github.com/fksegundo/rinha-dotnetrust-lb/actions/workflows/publish-image.yml)
[![GHCR image](https://img.shields.io/badge/GHCR-rinha--api--lb-blue)](https://github.com/users/fksegundo/packages/container/package/rinha-api-lb)

Companion custom load balancer for the `rinha-dotnetrust` submission to the [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026) challenge.

This is a minimal, high-throughput TCP load balancer written in Rust. It accepts external HTTP traffic and forwards it to API backend instances over Unix Domain Sockets, with support for both standard TCP proxying and zero-copy Unix socket file-descriptor passing.

## Why a custom load balancer?

- Keep the gateway image under our control
- Decouple the main submission repository from balancer source code
- Publish our own image for the final `docker-compose.yml`
- Support FD passing (`SCM_RIGHTS`) for lightweight socket handoff to API processes

## Architecture

```text
client
  |
  v
TCP :PORT (SO_REUSEPORT, multi-worker)
  |
  v
LB process
  |
  |--[proxy mode]----> UDS connect -> backend
  |
  +--[fd-pass mode]--> SCM_RIGHTS -> backend handles socket directly
```

The LB binds to a TCP port with `SO_REUSEPORT` so the kernel distributes incoming connections across worker threads. Each worker runs a single-threaded Tokio runtime. Accepted sockets are dispatched to backends via round-robin with a bitwise mask (upstream count must be a power of 2).

## Modes of operation

### 1. TCP proxy mode (`UPSTREAMS`)

The LB reads the HTTP request headers from the client, detects `GET /ready` for health-check responses, connects to the selected backend Unix socket, and then proxies the full connection bidirectionally.

Backend selection retries up to 2 rounds with 150 ms connect timeout and 25 ms backoff.

### 2. FD passing mode (`FD_UPSTREAMS`)

The LB peeks the first bytes of the TCP stream to detect `GET /ready`. For normal requests, it converts the Tokio `TcpStream` to a standard `std::net::TcpStream`, connects a control Unix socket to the backend, and sends the client socket file descriptor using `SCM_RIGHTS` via `sendmsg`. The backend process receives the FD and handles the HTTP connection directly.

This avoids copying data through the LB process after the handoff.

## Configuration

All settings are passed via environment variables:

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `UPSTREAMS` | yes* | — | Comma-separated UDS paths for proxy mode |
| `FD_UPSTREAMS` | yes* | — | Comma-separated UDS paths for FD-passing mode |
| `PORT` | no | `8080` | TCP port to listen on |
| `WORKERS` | no | `1` | Number of worker threads (each gets its own `SO_REUSEPORT` listener) |
| `RINHA_LB_DIAG` | no | `0` | Set to `1` to enable diagnostic error logging |

*One of `UPSTREAMS` or `FD_UPSTREAMS` must be provided.

The number of upstream entries must be a power of 2 for optimal round-robin scheduling.

## Endpoints handled by the LB

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/ready` | Readiness probe. The LB checks all backends with a 500 ms UDS connect timeout and returns `200 ok` or `503 Service Unavailable` |

## Local build

```bash
docker build \
  -t filonsegundo/rinha-dotnetrust-lb:submission \
  .
```

## Publish

```bash
LB_IMAGE=filonsegundo/rinha-dotnetrust-lb:submission \
./scripts/publish-image.sh
```

## Build variables

| Variable | Default | Description |
| --- | --- | --- |
| `RUST_TARGET_CPU` | `haswell` | Target CPU for `rustc` codegen |
| `LB_IMAGE` | `filonsegundo/rinha-dotnetrust-lb:submission` | Image tag used by the publish script |

## Implementation notes

### SO_REUSEPORT worker distribution

Instead of a single listener shared across threads, each worker creates its own `SO_REUSEPORT` socket. The kernel load-balances incoming TCP connections across them, eliminating accept-queue contention.

### Minimal HTTP parsing

The proxy mode parses just enough HTTP to detect `GET /ready` and find the end of the headers (`\r\n\r\n`). Everything else is forwarded as raw bytes.

### Health check

The `/ready` endpoint is handled by the LB itself. It attempts to connect to every configured backend Unix socket within 500 ms. Only if all succeed does it return `200 OK`.

## Project layout

```text
src/
  main.rs                LB entrypoint: listener setup, accept loop, proxy and FD-pass handlers

Dockerfile               Multi-stage build with cargo-chef for layer caching
```

## Stack

- Rust
- Tokio (single-threaded runtime per worker)
- socket2 (for `SO_REUSEPORT`)
- libc (for `SCM_RIGHTS` FD passing)

## Related repositories

- [fksegundo/rinha-rust](https://github.com/fksegundo/rinha-rust) — main submission this load balancer serves
- [zanfranceschi/rinha-de-backend-2026](https://github.com/zanfranceschi/rinha-de-backend-2026) — official challenge repository

## Acknowledgments

Some ideas for this load balancer were inspired by the approach used in [jairoblatt/SoNoForevis](https://github.com/jairoblatt/SoNoForevis).
