# Release Checklist — v{VERSION}

> Complete ALL items before tagging a release. No exceptions.
> For public releases, prepare the release from the parent source of truth first,
> then export and merge the public snapshot before publishing public tags or
> assets.

## Automated Gates (must be green)
- [ ] Quick suite (PR CI) — all green
- [ ] `release-smoke.yml` — last run green on branch head
- [ ] cargo-mutants score ≥ 70% on maekon-core
- [ ] Zero P0/P1 flaky tests in quarantine
- [ ] Public repository checks for the exported snapshot are green
- [ ] Required public repository Actions secrets for the intended release scope are configured
- [ ] No open Dependabot or CodeQL finding affects shipped release artifacts, or each remaining finding is explicitly accepted in the release record

## Manual Verification
- [ ] `cargo build --release` succeeds on macOS
- [ ] `cargo build --release` succeeds on Windows (or cross-compile)
- [ ] App launches and shows Dashboard with real data
- [ ] Settings save/load round-trip works
- [ ] Auto-updater detects the new version (staging)

## Test Layers Verification
- [ ] Layer 1 (Rust): `cargo test --workspace` — 0 failures
- [ ] Layer 2 (Mock IPC): `pnpm test` — 0 failures
- [ ] Layer 3 (Playwright): `pnpm test:e2e` — 0 failures
- [ ] Layer 4 (Tauri WDIO): `run-e2e-tauri.sh` — 0 failures

## Parent/Public Source Boundary
- [ ] Release-prep commit was created from `clients/maekon-client` in parent
- [ ] `tools/public-export/maekon-client/export.sh --dry-run --worktree` passed
- [ ] Public export PR merged into `pseudotop/maekon-client`
- [ ] Public tag points at the reviewed public repository state
- [ ] Parent source SHA and public export SHA are recorded in the release notes

## Artifact Integrity
- [ ] GitHub Release exists under `pseudotop/maekon-client`
- [ ] Every downloadable artifact has a matching `.sha256` sidecar
- [ ] Signature sidecars are present when signature verification is advertised
- [ ] macOS artifacts are signed, notarized, and stapled when applicable
- [ ] Installer smoke uses `pseudotop/maekon-client` release URLs
- [ ] Updater smoke uses the public repository/channel, not legacy `maekon-client`

Required Actions secrets for the public repository:

Required for public RC release:

- `MACOS_APP_CERT_P12_B64`
- `MACOS_APP_CERT_PASSWORD`
- `MACOS_APP_SIGNING_IDENTITY`
- `MACOS_INSTALLER_CERT_P12_B64`
- `MACOS_INSTALLER_CERT_PASSWORD`
- `MACOS_INSTALLER_SIGNING_IDENTITY`
- `MACOS_NOTARY_APPLE_ID`
- `MACOS_NOTARY_APP_PASSWORD`
- `MACOS_NOTARY_TEAM_ID`
- `UPDATE_SIGNING_PRIVATE_KEY_B64`

Required before stable promotion:

- `STABLE_RELEASE_DEPLOY_KEY`
- writable deploy key corresponding to `STABLE_RELEASE_DEPLOY_KEY`

## Documentation
- [ ] CHANGELOG.md updated
- [ ] Breaking changes documented (if any)
- [ ] Release notes and install docs explain that Maekon is the app display name while `maekon-*` artifacts and the `maekon` CLI command remain compatibility identifiers
- [ ] If this is the first public release, README/install docs no longer say release assets are unavailable

## Sign-off
- [ ] Maintainer approval
- [ ] Release created via `./scripts/release.sh <VERSION>` (RC) or `./scripts/promote-stable.sh <RC-VERSION>` (stable promotion). **Do NOT use `git tag` directly** — `release.sh` synchronizes `CHANGELOG.md`, `Cargo.toml`, and `tauri.conf.json`, then creates the tag via verified signing.
