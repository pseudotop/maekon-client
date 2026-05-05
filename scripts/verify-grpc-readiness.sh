#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CARGO_CMD="$ROOT_DIR/scripts/cargo-cache.sh"
cd "${ROOT_DIR}"

echo "[grpc] Verifying protobuf compatibility policy"
./scripts/verify-grpc-compatibility.sh

echo "[grpc] Verifying TLS/mTLS configuration policy"
./scripts/verify-grpc-mtls-config.sh

echo "[grpc] Checking maekon-network with grpc feature"
"$CARGO_CMD" check -p maekon-network --features grpc

echo "[grpc] Running maekon-network tests with grpc feature"
"$CARGO_CMD" test -p maekon-network --features grpc

echo "[grpc] Running stream reconnect/backpressure conformance tests"
"$CARGO_CMD" test -p maekon-network --features grpc reconnect_

echo "[grpc] Running stream chaos conformance tests"
"$CARGO_CMD" test -p maekon-network --features grpc chaos_

echo "[grpc] Running proxy harness fault-injection conformance tests"
"$CARGO_CMD" test -p maekon-network --features grpc proxy_fault_

echo "[grpc] Checking maekon-app wiring"
"$CARGO_CMD" check -p maekon-app --features maekon-network/grpc

echo "[grpc] Verifying committed generated proto files are up-to-date"
if ! git diff --quiet -- crates/maekon-network/src/proto/generated; then
  echo "Generated proto files changed. Regenerate and commit updated files:" >&2
  git diff -- crates/maekon-network/src/proto/generated >&2
  exit 1
fi

echo "[grpc] Readiness checks completed successfully"
