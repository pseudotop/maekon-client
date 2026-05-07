# Source Build Prerequisites

Use this guide when validating a fresh checkout of `pseudotop/maekon-client`.
The source tree intentionally does not commit generated frontend artifacts or
Tauri sidecar binaries, so a few checks need preparation before they are useful.

## Source-Only Checks

These checks should pass directly from a fresh checkout:

```bash
cargo metadata --no-deps --format-version 1
./scripts/check-architecture-deps.sh
./scripts/check-config-sync.sh
./scripts/check-language.sh i18n
```

`check-config-sync.sh` validates source configuration by default. It does not
require `crates/maekon-web/frontend/dist/` unless `--require-artifacts` is
passed. With `--require-artifacts`, the check requires `dist/index.html` plus
at least one generated JavaScript bundle, so a placeholder HTML file is not a
release-quality frontend artifact.

## Frontend Artifact Checks

Build the frontend before running checks that need Tauri `frontendDist`:

```bash
cd crates/maekon-web/frontend
pnpm install --frozen-lockfile
pnpm build
cd ../../..

./scripts/check-config-sync.sh --require-artifacts
```

## Tauri ExternalBin Checks

Tauri resolves `externalBin = ["maekon-sandbox-worker"]` at compile time. Local
Tauri tests that compile `maekon-app` need a host-triple sidecar file under
`src-tauri/`.

For compile-only smoke tests, create an ignored placeholder:

```bash
TRIPLE="$(rustc -vV | awk '/^host:/ { print $2 }')"
touch "src-tauri/maekon-sandbox-worker-$TRIPLE"
chmod +x "src-tauri/maekon-sandbox-worker-$TRIPLE"

./scripts/cargo-cache.sh test -p maekon-app --test release_smoke_hygiene
```

For release packaging, do not use the placeholder. Build the real sidecar:

```bash
TRIPLE="$(rustc -vV | awk '/^host:/ { print $2 }')"
./scripts/cargo-cache.sh build -p maekon-sandbox-worker
cp target/debug/maekon-sandbox-worker "src-tauri/maekon-sandbox-worker-$TRIPLE"
chmod +x "src-tauri/maekon-sandbox-worker-$TRIPLE"
```

The generated sidecar path is ignored by `.gitignore`. It should not be
committed.
