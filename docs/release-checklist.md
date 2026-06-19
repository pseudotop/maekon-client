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
- [ ] Public branch `CI` was manually dispatched for the exported branch when the change affects Rust, CI, release scripts, or packaged artifacts; all `Build (${{ matrix.target }})` rows are green
- [ ] Fresh-checkout source checks follow `docs/testing/source-build-prerequisites.md`
- [ ] `./scripts/check-config-sync.sh --require-artifacts` passes after `pnpm build` (or
  `MAEKON_RELEASE_REQUIRE_ARTIFACTS=1 ./scripts/pre-release-check.sh <VERSION>` is run
  from a checkout with frontend artifacts already built)
- [ ] `MAEKON_RELEASE_DECISION_MANIFEST=<manifest.json> ./scripts/pre-release-check.sh <VERSION>`
  passes from the exact commit to be tagged; the manifest `release_tag`,
  `commit_sha`, and `release_decision.state=pass` must match the release.
- [ ] Required public repository Actions secrets for the intended release scope are configured
- [ ] Public repository PR, issue, Dependabot, and CodeQL queues were triaged immediately before release/export merge
- [ ] No open Dependabot or CodeQL finding affects shipped release artifacts, or each remaining finding is explicitly accepted in `supply-chain/release-alert-acceptance.json`
- [ ] Provider-owned CLI compatibility gate passes:
  `provider_specs::tests::subprocess_compatibility_matrix_matches_e18_release_gate_contract`,
  `provider_specs::tests::rejects_subprocess_surface_without_compatibility_matrix`, and
  `provider_specs::tests::subprocess_output_contracts_match_e18_matrix`

## Manual Verification
- [ ] `cargo build --release` succeeds on macOS
- [ ] `cargo build --release` succeeds on Windows (or cross-compile)
- [ ] App launches and shows Dashboard with real data
- [ ] Settings save/load round-trip works
- [ ] Auto-updater detects the new version (staging)
- [ ] Provider-owned CLI live smoke is recorded for each preferred headless CLI
  surface using the privacy-safe checklist in
  `docs/qa/provider-cli-compatibility-matrix.md`
- [ ] E19 desktop smoke release-decision manifest is generated and accepted
  before final sign-off; it must include History-First evidence mapping for
  every release-critical claim and must reject missing, stale, incomplete, or
  privacy-blocked evidence.

## Test Layers Verification
- [ ] Layer 1 (Rust): `cargo test --workspace` — 0 failures
- [ ] Layer 2 (Mock IPC): `pnpm test` — 0 failures
- [ ] Layer 3 (Playwright): `pnpm test:e2e` — 0 failures
- [ ] Layer 4 (Tauri WDIO): `run-e2e-tauri.sh` — 0 failures

## Parent/Public Source Boundary
- [ ] Release-prep commit was created from `clients/maekon-client` in parent
- [ ] Parent repository PR for the release/export change is merged before the public export PR is marked ready or merged
- [ ] Internal export dry-run passed from the parent source tree: `clients/maekon-client/scripts/export-public-repo.sh --dry-run --worktree`
- [ ] Public export was generated from the merged parent source SHA, not from an unmerged local-only branch
- [ ] Public export PR was opened by the Maekon GitHub App/bot via `public-export-pr.yml` or `update-public-repo-clone.sh --open-pr`, not by the maintainer user who must approve it
- [ ] Any early public PR used for CI preview stayed draft until the merged parent source SHA was re-exported or confirmed to produce no public diff
- [ ] Public export PR merged into `pseudotop/maekon-client`
- [ ] Public pull-request CI alone was not used as cross-platform build proof; for runtime/build-impacting exports, the public branch has a successful manual `CI` `workflow_dispatch` run
- [ ] Public `main` branch protection or an active ruleset is enabled before tagging
- [ ] Public tag is an annotated signed tag and GitHub reports tag signature verification as passing
- [ ] Public tag points at the reviewed public repository state
- [ ] Parent source SHA and public export SHA are recorded in the release notes

## Artifact Integrity
- [ ] GitHub Release exists under `pseudotop/maekon-client`
- [ ] Every downloadable artifact has a matching `.sha256` sidecar
- [ ] Signature sidecars are present when signature verification is advertised
- [ ] macOS artifacts are signed, notarized, and stapled when applicable
- [ ] The final notarized bytes for `maekon-macos-universal.dmg` and
  `maekon-macos-universal.pkg` were republished with regenerated `.sha256`,
  `.sig`, and provenance after stapling
- [ ] `notarization-final-byte-manifest.json` is present and records the same
  SHA-256 digests as the final downloadable macOS installers
- [ ] Release Guard accepted the current macOS release assets and would reject a
  stale checksum or stale signature sidecar
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
- `MAEKON_UPDATE_PUBLIC_KEY`

Required before stable promotion:

- `MAEKON_RELEASE_APP_CLIENT_ID`
- `MAEKON_RELEASE_APP_PRIVATE_KEY`

The release App installation must be able to read repository security alerts
used by `release.yml` (`Dependabot alerts` and `Code scanning alerts`). If the
App permission update is still pending, use a short-lived
`MAEKON_RELEASE_SECURITY_TOKEN` repository secret with equivalent read access
for the release rerun, then remove it after the release completes.

Optional but recommended for release freshness:

- `MAEKON_LANDING_DEPLOY_HOOK`

Required before AI live smoke dispatch:

- `MAEKON_AI_SMOKE_LLM_ENDPOINT`
- `MAEKON_AI_SMOKE_LLM_API_KEY`
- `MAEKON_AI_SMOKE_LLM_MODEL`
- `MAEKON_AI_SMOKE_OCR_ENDPOINT`
- `MAEKON_AI_SMOKE_OCR_API_KEY`
- `MAEKON_AI_SMOKE_OCR_MODEL`

## Documentation
- [ ] CHANGELOG.md contains a curated `## [<version>] - YYYY-MM-DD` section that passes `scripts/verify-release-notes-policy.sh --public --version <version>`
- [ ] Breaking changes documented (if any)
- [ ] Release notes and install docs explain that Maekon is the app display name while `maekon-*` artifacts and the `maekon` CLI command remain compatibility identifiers
- [ ] Release notes mention provider-owned CLI drift diagnostics: update the
  provider CLI, restart Maekon when Settings reports a stale process
  environment, refresh Support Diagnostics, and include only sanitized provider
  CLI diagnostics in bug reports
- [ ] If this is the first public release, README/install docs no longer say release assets are unavailable

## Sign-off
- [ ] Maintainer approval
- [ ] RC release created via `./scripts/release.sh <VERSION>` followed by `./scripts/publish-rc-tag.sh <VERSION>`.
- [ ] Stable release created by running `promote-stable.yml` to open a stable promotion PR, merging that PR into `main`, then running `./scripts/publish-stable-tag.sh <VERSION>` from latest `main`.
- [ ] **Do NOT use `git tag` directly** — the publish scripts synchronize release checks and create signed annotated tags that the release workflow verifies through GitHub.
