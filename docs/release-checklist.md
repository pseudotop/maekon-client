# Release Checklist — v{VERSION}

> Complete ALL items before tagging a release. No exceptions.
> For public releases, prepare the release from the parent source of truth first,
> then export and merge the public snapshot before publishing public tags or
> assets.

## Automated Gates (must be green)
- [ ] Quick suite (PR CI) — all green
- [ ] `release-smoke.yml` — last run green on branch head
- [ ] cargo-mutants score ≥ 70% on maekon-core, proven by a linked
  `Maekon Mutation Score` (`maekon-client-mutants.yml`) aggregate run. Record the
  run URL here. The run's receipt must satisfy ALL of:
  - `source_sha` equals the exact commit to be tagged
  - `scope` is empty (whole crate) — a scoped run is NOT a crate score and the
    receipt records it as such
  - the aggregate job succeeded, which means the shard set was complete
    (`enumerated == merged_total`) and the viable score cleared the threshold
  This box must not be ticked from a local run, a partial run, a scoped run, or
  a run at a different SHA (#10003).
- [ ] Zero P0/P1 flaky tests in quarantine
- [ ] Public repository checks for the exported snapshot are green
- [ ] Public export provenance manifest was generated and verified from the
  exact merged parent source SHA: `.maekon-public-export-provenance.json`
- [ ] Trusted public export verification used a signed source binding
  (`MAEKON_REQUIRE_PUBLIC_EXPORT_PROVENANCE_SIGNATURE=1` with
  `MAEKON_UPDATE_PUBLIC_KEY`) or an equivalent GitHub artifact attestation
  before treating the manifest `ssot.source_sha` as authoritative
- [ ] Public branch `CI` was manually dispatched for the exported branch when the change affects Rust, CI, release scripts, or packaged artifacts; all `Build (${{ matrix.target }})` rows are green
- [ ] All four `Build (${{ matrix.target }})` rows are green **on the exact commit
  to be tagged**, not merely on the export PR head. These build the shipped
  feature set (`stt,download,lan-sync` + the per-OS sandbox feature) and run the
  Windows PE closure check on the binary that actually ships. If the commit has
  no Build matrix, dispatch `Build Smoke Test` with `ref=<exact SHA>`; it
  publishes the same check-run names and the same closure verification.
  `pre-release-check.sh` enforces this and fails closed (#10698: v0.0.1-rc.7 was
  tagged while this gate ran only against a `--features grpc` binary, so
  `maekon.exe -> mmdevapi.dll` first appeared in `release.yml` — after the tag
  was irreversible, leaving a signed tag with no artifacts).
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
- [ ] `./scripts/check-webdriver-security-isolation.sh` passes, proving any
  accepted GTK3/glib finding remains confined to the optional WDIO test harness
  and is absent from the exact shipped Linux feature graph
- [ ] Provider-owned CLI compatibility gate passes:
  `provider_specs::tests::subprocess_compatibility_matrix_matches_e18_release_gate_contract`,
  `provider_specs::tests::rejects_subprocess_surface_without_compatibility_matrix`, and
  `provider_specs::tests::subprocess_output_contracts_match_e18_matrix`
- [ ] Windows release binaries link OpenSSL **vendored from source** via
  `openssl-src` (`rusqlite`'s `bundled-sqlcipher-vendored-openssl`), NOT a
  system OpenSSL SDK. Plain `bundled-sqlcipher` links the system OpenSSL
  *dynamically* on Windows, which is what shipped rc.6 without
  `libcrypto-3-x64.dll` (#9884). The Windows setup action must not export
  `OPENSSL_DIR`/`OPENSSL_LIB_DIR`/`OPENSSL_LIBS`/`OPENSSL_STATIC`/
  `OPENSSL_NO_VENDOR` — any of them defeats vendoring
  (`scripts/test-release-workflow-governance.sh` enforces this).
- [ ] The `openssl-src` exemption in `supply-chain/config.toml` still covers the
  vendored version in `Cargo.lock` and its `review-by` date has not passed
  (currently `300.5.5+3.5.5`, review-by 2027-02-04). Vendoring builds OpenSSL
  from source, so a version bump changes what actually ships.
- [ ] Any
  residual retail Microsoft VC runtime imports are staged from the signed
  Visual Studio redistributable directory into the application-local payload.
  The PE import-closure validator passes independently for the
  prebuilt payload, ZIP, MSI administrative extraction, and NSIS extraction
  (`node scripts/verify-windows-runtime-closure.mjs ...`).

## Manual Verification
- [ ] `cargo build --release` succeeds on macOS
- [ ] `cargo build --release` succeeds on Windows (or cross-compile)
- [ ] On a clean Windows host without OpenSSL or developer tools, install the
  MSI and NSIS packages in turn; record successful first launch, sandbox-worker
  startup, uninstall, and the explicit keep/remove user-data choice for each.
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
- [ ] Public export provenance records the parent SSOT source SHA, client
  subtree SHA, generated export snapshot SHA-256, public repository target SHA,
  public content diff result, and `source_binding` status
- [ ] Unsigned public export provenance was treated as advisory only; source
  SHA trust came from a verified Ed25519 `source_binding.signature` or GitHub
  artifact attestation
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
- [ ] Release bundle contains `sbom.cdx.json` and `sbom.cdx.json.sha256`; the
  checksum verifies against the generated SBOM
- [ ] Signature sidecars are present when signature verification is advertised
- [ ] macOS artifacts are signed, notarized, and stapled when applicable
- [ ] Stapling was confirmed **on the published release assets**, not on the
  notarization run's own output:
  `./scripts/verify-published-macos-notarization.sh <TAG>`. A green Release
  workflow does not imply notarization succeeded — `Notarize macOS Release
  Assets` runs separately and can fail on its own (#10935)
- [ ] The final notarized bytes for `maekon-macos-universal.dmg` and
  `maekon-macos-universal.pkg` were republished with regenerated `.sha256`,
  `.sig`, and provenance after stapling
- [ ] `notarization-final-byte-manifest.json` is present and records the same
  SHA-256 digests as the final downloadable macOS installers
- [ ] Release Guard accepted the current macOS release assets and would reject a
  stale checksum or stale signature sidecar
- [ ] GitHub artifact attestations/provenance exist for the final `dist/*`
  release subjects, and the release workflow evidence points to the current tag
  commit rather than an older export or build rerun
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

The release App installation on `pseudotop/maekon-client` must grant
`Contents: write`, `Pull requests: write`, `Dependabot alerts: read`, and
`Code scanning alerts: read`. Release workflows request narrower
installation-token permissions explicitly per job: public export PR creation
uses `Contents: read` + `Pull requests: write`, stable-promotion PR preparation
uses `Contents: write` + `Pull requests: write`, and release verification uses
`Contents: read` + security-alert read permissions. Missing GitHub App
permissions fail during token generation instead of failing later inside `gh`
commands.

`MAEKON_RELEASE_SECURITY_TOKEN` may still override the security-alert read token
for an emergency release rerun, but the release App installation itself must
also carry the alert-read permissions above.

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
- [ ] Final release notes distinguish the exported source state from the
  published binary release state: parent SSOT SHA, public export snapshot SHA,
  public repository commit/tag, SBOM checksum, and artifact attestation evidence
  are listed separately
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
- [ ] If a tag has already been pushed and the release job failed, follow `docs/guides/release-tag-recovery.md` before deleting anything. Whether any asset was published is what decides the procedure, and it must be checked first — `v0.0.1-rc.9` was recovered by replacing the tag only because the failure happened before publication.
