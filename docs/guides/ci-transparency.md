# CI/CD Transparency

This page documents what runs on every pull request and how releases are produced. It is intended for enterprise buyers performing due diligence and for contributors reading build results.

---

## What Runs on Every Pull Request

The CI pipeline is defined in [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml).

### Static Analysis (runs first)

| Step | Command | Blocks merge on failure |
|------|---------|------------------------|
| Format check | `cargo fmt --all -- --check` | Yes |
| Clippy (standalone) | `cargo clippy --workspace --all-targets -- -D warnings` | Yes |
| Clippy (server features) | `cargo clippy --workspace --all-targets --features server` | Yes |
| Clippy (gRPC features) | `cargo clippy --workspace --all-targets --features grpc` | Yes |
| Web contract boundary | `scripts/verify-web-contract-boundary.sh` | Yes |
| HTTP interface manifest | `scripts/verify-http-interface-manifest.sh` | Yes |
| Commit message hygiene | `scripts/verify-commit-message-hygiene.sh` | Yes |

`RUSTFLAGS=-Dwarnings` is set globally, so any compiler warning is a build failure.

### Tests

| Step | Command |
|------|---------|
| Standalone | `cargo test --workspace` |
| Server features | `cargo test --workspace --features server` |
| gRPC features | `cargo test --workspace --features grpc` |

The `Test` check is always emitted on pull requests so branch protection can require it consistently. When a PR only changes frontend files, the job exits quickly with a no-op success instead of running the full Rust suite.

### Release Smoke (post-merge / manual)

Release-grade desktop smoke is intentionally separated from the fast PR lane. The workflow lives in [`.github/workflows/release-smoke.yml`](../../.github/workflows/release-smoke.yml) and runs on pushes to `main` / `develop` or via manual dispatch.

After the fast PR lane merges, a release build is compiled on all four targets to catch platform-specific compilation errors:

| Target | Runner |
|--------|--------|
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` |
| `aarch64-apple-darwin` | `macos-latest` |
| `x86_64-apple-darwin` | `macos-14` |
| `x86_64-pc-windows-msvc` | `windows-latest` |

A GUI bootstrap smoke test runs on macOS and Windows: the binary is launched for 3 seconds and inspected for Rust panics or tokio runtime failures.

This split keeps the merge-blocking PR checks focused on fast feedback while preserving release-grade platform coverage after merge and before promotion.

### Frontend (web dashboard)

If frontend files change, a separate job runs:

1. `pnpm install --frozen-lockfile`
2. `pnpm build`
3. Playwright E2E tests (Chromium headless)

---

## How to Read CI Output

1. Open the PR on GitHub.
2. Scroll to the "Checks" section at the bottom.
3. Green checkmarks indicate passing jobs. Click a failing job name to open the run log.
4. The most common failures are:
   - **fmt**: Run `cargo fmt` locally and push.
   - **clippy**: Read the `error[clippy::...]` line in the log and fix the lint.
   - **test**: The failing test name is printed before the panic/assertion output.

Logs are available for 90 days after a run. Artifacts (frontend dist, GUI smoke logs) are retained for 14 days.

---

## Release Pipeline

Releases are driven by tags, but the repository now enforces an `RC first, stable promote later` flow:

- RC publish: push `vX.Y.Z-rc.N` after the RC preparation PR is merged
- Stable publish: run `Promote Stable Release` for that validated RC; maintainers do not push `vX.Y.Z` manually

The release workflow is defined in [`.github/workflows/release.yml`](../../.github/workflows/release.yml). Maintainers should use:

- `./scripts/release.sh <x.y.z-rc.N>` to prepare the RC version/changelog commit on a PR branch
- merge that PR into `main`
- `./scripts/publish-rc-tag.sh <x.y.z-rc.N>` on the merged `main` commit to publish the RC tag
- `./scripts/promote-stable.sh <x.y.z-rc.N>` to create the stable promotion commit and tag locally for verification or by CI with `PROMOTE_STABLE_NO_PUSH=1`
- [`.github/workflows/promote-stable.yml`](../../.github/workflows/promote-stable.yml) to let GitHub Actions create the stable tag and dispatch the release build without a human pushing the stable tag manually
- [`.github/workflows/release-guard.yml`](../../.github/workflows/release-guard.yml) automatically deletes manual GitHub releases that bypass the workflow path or publish without assets

Direct stable preparation via the old `prepare-release.sh` path is intentionally blocked.

### Pre-flight Checks

Before any build starts, the pipeline verifies:

- The tag format is either `vX.Y.Z-rc.N` or `vX.Y.Z`.
- The tag points to the current `main` branch head.
- `Cargo.toml` workspace version matches the git tag.
- `Cargo.lock` workspace package versions match the git tag.
- `crates/oneshim-web/frontend/package.json` matches the git tag.
- `CHANGELOG.md` contains exactly one `[Unreleased]` header and an entry for the version being released.
- `tauri.conf.json` `productName` is `ONESHIM` and `identifier` is `com.oneshim.client`.

For stable tags, the pipeline also verifies:

- A matching RC tag already exists.
- The stable commit changes metadata files only.
- The stable changelog section matches the latest RC section for that base version.
- The matching RC GitHub release exists as a prerelease and already has assets.

### Build Matrix

| Platform | Target | Artifact |
|----------|--------|----------|
| macOS (Apple Silicon) | `aarch64-apple-darwin` | `.dmg` (notarized) |
| macOS (Intel) | `x86_64-apple-darwin` | `.dmg` (notarized) |
| Windows | `x86_64-pc-windows-msvc` | `.msi` (signed) |
| Linux (Debian) | `x86_64-unknown-linux-gnu` | `.deb` |
| Linux (RPM) | `x86_64-unknown-linux-gnu` | `.rpm` |

macOS `.dmg` files are notarized via Apple's notarization service using the workflow defined in [`.github/workflows/notarize-macos-release-assets.yml`](../../.github/workflows/notarize-macos-release-assets.yml). Gatekeeper will allow installation without warnings.

### GitHub Release

After all builds succeed, a GitHub Release is created automatically with:
- Changelog entry for the version.
- All platform artifacts attached.
- SHA-256 checksums for each artifact.

---

## Self-Update Mechanism

ONESHIM checks GitHub Releases for updates using the updater module in `src-tauri/src/updater/`. The update flow:

1. On a configurable interval, the updater fetches the GitHub Releases API.
2. The latest release version is compared against the running version using semver.
3. A minimum allowed version floor (`update.min_allowed_version`) prevents downgrades below a security baseline.
4. If a newer version is found, the user is prompted via a desktop notification.
5. On approval, the installer is downloaded, its signature verified, and the update applied.

All update downloads are verified with signature checking. Signature verification can only be disabled if `update.require_signature_verification` is explicitly set to `false` in config; the CI `integrity-gates` workflow enforces that this field remains `true` in production configurations.

---

## Additional CI Workflows

| Workflow | Trigger | Purpose |
|---------|---------|---------|
| `integrity-gates.yml` | Push to main / manual / schedule | Integrity policy + dependency audit |
| `security-compliance.yml` | Push to main / manual / schedule | Supply-chain controls + SBOM |
| `release-smoke.yml` | Push to main / manual | Cross-platform desktop release smoke |
| `grpc-governance.yml` | Push to main | gRPC contract stability |
| `ai-integration-smoke.yml` | Push to main | AI provider integration smoke |
| `macos-windowserver-gui-smoke.yml` | Push to main | Full macOS GUI smoke with WindowServer |
