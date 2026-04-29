#!/usr/bin/env bash
# Regenerate gRPC client code from Consumer Contract proto definitions.
#
# Prerequisites:
#   cargo install tonic-prost-build-cli   # or have protoc + tonic-prost-build available
#
# Usage:
#   ./scripts/regenerate-protos.sh
#
# The generated file is committed to git so that builds don't require protoc.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_CMD="$ROOT_DIR/scripts/cargo-cache.sh"
PROTO_ROOT="$ROOT_DIR/api/proto"
OUT_DIR="$ROOT_DIR/crates/oneshim-network/src/proto/generated"

PROTOS=(
  "$PROTO_ROOT/oneshim/client/v1/auth.proto"
  "$PROTO_ROOT/oneshim/client/v1/session.proto"
  "$PROTO_ROOT/oneshim/client/v1/context.proto"
  "$PROTO_ROOT/oneshim/client/v1/suggestion.proto"
  "$PROTO_ROOT/oneshim/client/v1/health.proto"
)

# Verify all proto files exist
for proto in "${PROTOS[@]}"; do
  if [ ! -f "$proto" ]; then
    echo "ERROR: Proto file not found: $proto"
    exit 1
  fi
done

mkdir -p "$OUT_DIR"

echo "Compiling Consumer Contract protos..."

# Use a temporary Cargo project to run tonic-prost-build
TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT

cat > "$TEMP_DIR/Cargo.toml" <<'CARGO'
[package]
name = "proto-gen"
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
        .build_server(false)
        .build_client(true)
        .out_dir(&out_dir)
        .compile_protos(&protos, std::slice::from_ref(&proto_root))
        .expect("Failed to compile protos");
}
BUILDRS

echo "Running code generation (this downloads tonic-prost-build if needed)..."
"$CARGO_CMD" build --manifest-path "$TEMP_DIR/Cargo.toml" --quiet 2>&1

echo "Generated: $OUT_DIR/oneshim.client.v1.rs"
echo ""
echo "Don't forget to commit the updated generated file:"
echo "  git add crates/oneshim-network/src/proto/generated/"
echo "  git commit -m 'chore: regenerate Consumer Contract proto code'"
