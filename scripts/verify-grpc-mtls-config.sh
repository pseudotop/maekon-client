#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CARGO_CMD="$ROOT_DIR/scripts/cargo-cache.sh"
cd "${ROOT_DIR}"

echo "[grpc-mtls] Validating TLS/mTLS configuration invariants"
"$CARGO_CMD" test -p oneshim-network --features grpc test_tls_
"$CARGO_CMD" test -p oneshim-network --features grpc test_mtls_

echo "[grpc-mtls] TLS/mTLS validation checks completed"
