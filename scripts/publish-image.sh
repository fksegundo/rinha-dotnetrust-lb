#!/usr/bin/env bash
set -euo pipefail

LB_IMAGE="${LB_IMAGE:-fksegundo/rinha-dotnetrust-lb:latest}"
RUST_TARGET_CPU="${RUST_TARGET_CPU:-haswell}"

docker buildx build \
  --platform linux/amd64 \
  --build-arg RUST_TARGET_CPU="${RUST_TARGET_CPU}" \
  -t "${LB_IMAGE}" \
  --push \
  .
