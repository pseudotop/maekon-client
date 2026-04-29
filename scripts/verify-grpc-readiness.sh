#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CARGO_CMD="$ROOT_DIR/scripts/cargo-cache.sh"
cd "${ROOT_DIR}"

echo "[grpc] Verifying protobuf compatibility policy"
./scripts/verify-grpc-compatibility.sh

echo "[grpc] Verifying TLS/mTLS configuration policy"
./scripts/verify-grpc-mtls-config.sh

echo "[grpc] Checking oneshim-network with grpc feature"
"$CARGO_CMD" check -p oneshim-network --features grpc

echo "[grpc] Running oneshim-network tests with grpc feature"
"$CARGO_CMD" test -p oneshim-network --features grpc

echo "[grpc] Running stream reconnect/backpressure conformance tests"
"$CARGO_CMD" test -p oneshim-network --features grpc reconnect_

echo "[grpc] Running stream chaos conformance tests"
"$CARGO_CMD" test -p oneshim-network --features grpc chaos_

echo "[grpc] Running proxy harness fault-injection conformance tests"
"$CARGO_CMD" test -p oneshim-network --features grpc proxy_fault_

echo "[grpc] Checking oneshim-app wiring"
"$CARGO_CMD" check -p oneshim-app --features oneshim-network/grpc

echo "[grpc] Verifying committed generated proto files are up-to-date"
if ! git diff --quiet -- crates/oneshim-network/src/proto/generated; then
  echo "Generated proto files changed. Regenerate and commit updated files:" >&2
  git diff -- crates/oneshim-network/src/proto/generated >&2
  exit 1
fi

echo "[grpc] Readiness checks completed successfully"
