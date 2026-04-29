#!/usr/bin/env bash
# Regenerate gRPC SERVER code for the dashboard service (D13).
#
# Parallel to regenerate-protos.sh (which handles the client-side Consumer
# Contract for oneshim-network). This script generates SERVER trait code for
# oneshim-web to serve external CLI/integration tools.
#
# Prerequisites:
#   protoc in PATH (macOS: `brew install protobuf`)
#   OR let cargo install tonic-prost-build transitively on first run
#
# Usage:
#   ./scripts/regenerate-dashboard-protos.sh
#
# Generated code is committed to git so builds don't require protoc.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_CMD="$ROOT_DIR/scripts/cargo-cache.sh"
PROTO_ROOT="$ROOT_DIR/api/proto"
OUT_DIR="$ROOT_DIR/crates/oneshim-web/src/proto/generated"

PROTOS=(
  "$PROTO_ROOT/oneshim/dashboard/v1/dashboard.proto"
)

for proto in "${PROTOS[@]}"; do
  if [ ! -f "$proto" ]; then
    echo "ERROR: proto file missing: $proto" >&2
    exit 1
  fi
done

mkdir -p "$OUT_DIR"

echo "Compiling Dashboard gRPC protos (server-side)..."

TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT

cat > "$TEMP_DIR/Cargo.toml" <<'CARGO'
[package]
name = "proto-gen-dashboard"
version = "0.0.0"
edition = "2021"

[build-dependencies]
tonic-prost-build = "0.14"
CARGO

mkdir -p "$TEMP_DIR/src"
echo "fn main() {}" > "$TEMP_DIR/src/main.rs"

cat > "$TEMP_DIR/build.rs" <<BUILDRS
fn main() {
    let proto_root = std::path::PathBuf::from("$PROTO_ROOT");
    let protos: Vec<std::path::PathBuf> = vec![
$(for p in "${PROTOS[@]}"; do echo "        std::path::PathBuf::from(\"$p\"),"; done)
    ];
    let out_dir = std::path::PathBuf::from("$OUT_DIR");
    std::fs::create_dir_all(&out_dir).unwrap();

    tonic_prost_build::configure()
        .build_server(true)
        // D13-v2 integration test needs client stubs to exercise the server
        // end-to-end. Client code only links in when the test binary builds;
        // production release binaries tree-shake it if unused.
        .build_client(true)
        .out_dir(&out_dir)
        .compile_protos(&protos, std::slice::from_ref(&proto_root))
        .expect("Failed to compile dashboard protos");
}
BUILDRS

echo "Running code generation..."
"$CARGO_CMD" build --manifest-path "$TEMP_DIR/Cargo.toml" --quiet 2>&1

echo "Generated: $OUT_DIR/oneshim.dashboard.v1.rs"
echo ""
echo "Don't forget to commit the updated generated file:"
echo "  git add crates/oneshim-web/src/proto/generated/"
echo "  git commit -m 'chore: regenerate Dashboard gRPC server code'"
