#!/usr/bin/env bash
set -euo pipefail

LB_IMAGE="${LB_IMAGE:-filonsegundo/rinha-dotnetrust-lb:submission}"
RUST_TARGET_CPU="${RUST_TARGET_CPU:-haswell}"

docker buildx build \
  --platform linux/amd64 \
  --build-arg RUST_TARGET_CPU="${RUST_TARGET_CPU}" \
  -t "${LB_IMAGE}" \
  -t "filonsegundo/rinha-dotnetrust-lb:latest" \
  --push \
  .
