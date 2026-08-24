# Release Checklist — v{VERSION}

> Complete ALL items before tagging a release. No exceptions.
> For public releases, prepare the release from the parent source of truth first,
> then export and merge the public snapshot before publishing public tags or
> assets.

## Automated Gates (must be green)
<!-- release-check-id: RC-AUTO-001 -->
- [ ] Quick suite (PR CI) — all green
<!-- release-check-id: RC-AUTO-002 -->
- [ ] `release-smoke.yml` — last run green on branch head
<!-- release-check-id: RC-AUTO-003 -->
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
<!-- release-check-id: RC-AUTO-004 -->
- [ ] Zero P0/P1 flaky tests in quarantine
<!-- release-check-id: RC-AUTO-005 -->
- [ ] Public repository checks for the exported snapshot are green
<!-- release-check-id: RC-AUTO-006 -->
- [ ] Public export provenance manifest was generated and verified from the
  exact merged parent source SHA: `.maekon-public-export-provenance.json`
<!-- release-check-id: RC-AUTO-007 -->
- [ ] Trusted public export verification used a signed source binding
  (`MAEKON_REQUIRE_PUBLIC_EXPORT_PROVENANCE_SIGNATURE=1` with
  `MAEKON_UPDATE_PUBLIC_KEY`) or an equivalent GitHub artifact attestation
  from `Maekon Public Export Source Attestation` before treating the manifest
  `ssot.source_sha` as authoritative. The receipt records the exact parent
  SSOT SHA, exact public commit SHA, workflow run URL, and attestation URL.
<!-- release-check-id: RC-AUTO-008 -->
- [ ] Public branch `CI` was manually dispatched for the exported branch when the change affects Rust, CI, release scripts, or packaged artifacts; all `Build (${{ matrix.target }})` rows are green
<!-- release-check-id: RC-AUTO-009 -->
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
<!-- release-check-id: RC-AUTO-010 -->
- [ ] Fresh-checkout source checks follow `docs/testing/source-build-prerequisites.md`
<!-- release-check-id: RC-AUTO-011 -->
- [ ] `./scripts/check-config-sync.sh --require-artifacts` passes after `pnpm build` (or
  `MAEKON_RELEASE_REQUIRE_ARTIFACTS=1 ./scripts/pre-release-check.sh <VERSION>` is run
  from a checkout with frontend artifacts already built)
<!-- release-check-id: RC-AUTO-012 -->
- [ ] `MAEKON_RELEASE_DECISION_MANIFEST=<manifest.json> ./scripts/pre-release-check.sh <VERSION>`
  passes from the exact commit to be tagged; the manifest `release_tag`,
  `commit_sha`, and `release_decision.state=pass` must match the release.
<!-- release-check-id: RC-AUTO-013 -->
- [ ] Required public repository Actions secrets for the intended release scope are configured
<!-- release-check-id: RC-AUTO-014 -->
- [ ] Public repository PR, issue, Dependabot, and CodeQL queues were triaged immediately before release/export merge
<!-- release-check-id: RC-AUTO-015 -->
- [ ] No open Dependabot or CodeQL finding affects shipped release artifacts, or each remaining finding is explicitly accepted in `supply-chain/release-alert-acceptance.json`
<!-- release-check-id: RC-AUTO-016 -->
- [ ] `./scripts/check-webdriver-security-isolation.sh` passes, proving any
  accepted GTK3/glib finding remains confined to the optional WDIO test harness
  and is absent from the exact shipped Linux feature graph
<!-- release-check-id: RC-AUTO-017 -->
- [ ] Provider-owned CLI compatibility gate passes:
  `provider_specs::tests::subprocess_compatibility_matrix_matches_e18_release_gate_contract`,
  `provider_specs::tests::rejects_subprocess_surface_without_compatibility_matrix`, and
  `provider_specs::tests::subprocess_output_contracts_match_e18_matrix`
<!-- release-check-id: RC-AUTO-018 -->
- [ ] Windows release binaries link OpenSSL **vendored from source** via
  `openssl-src` (`rusqlite`'s `bundled-sqlcipher-vendored-openssl`), NOT a
  system OpenSSL SDK. Plain `bundled-sqlcipher` links the system OpenSSL
  *dynamically* on Windows, which is what shipped rc.6 without
  `libcrypto-3-x64.dll` (#9884). The Windows setup action must not export
  `OPENSSL_DIR`/`OPENSSL_LIB_DIR`/`OPENSSL_LIBS`/`OPENSSL_STATIC`/
  `OPENSSL_NO_VENDOR` — any of them defeats vendoring
  (`scripts/test-release-workflow-governance.sh` enforces this).
<!-- release-check-id: RC-AUTO-019 -->
- [ ] The `openssl-src` exemption in `supply-chain/config.toml` still covers the
  vendored version in `Cargo.lock` and its `review-by` date has not passed
  (currently `300.5.5+3.5.5`, review-by 2027-02-04). Vendoring builds OpenSSL
  from source, so a version bump changes what actually ships.
<!-- release-check-id: RC-AUTO-020 -->
- [ ] Any
  residual retail Microsoft VC runtime imports are staged from the signed
  Visual Studio redistributable directory into the application-local payload.
  The PE import-closure validator passes independently for the
  prebuilt payload, ZIP, MSI administrative extraction, and NSIS extraction
  (`node scripts/verify-windows-runtime-closure.mjs ...`).

## Manual Verification
<!-- release-check-id: RC-MANUAL-001 -->
- [ ] `cargo build --release` succeeds on macOS
<!-- release-check-id: RC-MANUAL-002 -->
- [ ] `cargo build --release` succeeds on Windows (or cross-compile)
<!-- release-check-id: RC-MANUAL-003 -->
- [ ] On a clean Windows host without OpenSSL or developer tools, install the
  MSI and NSIS packages in turn; record successful first launch, sandbox-worker
  startup, uninstall, and the explicit keep/remove user-data choice for each.
<!-- release-check-id: RC-MANUAL-004 -->
- [ ] App launches and shows Dashboard with real data
<!-- release-check-id: RC-MANUAL-005 -->
- [ ] Settings save/load round-trip works
<!-- release-check-id: RC-MANUAL-006 -->
- [ ] **Post-publish gate:** a previous RC configured for the prerelease
  channel detects the newly published RC. Record this item as explicit
  `pending` in the pre-tag release-decision manifest, then validate the exact
  tag/commit-bound runtime receipt with
  `scripts/verify-post-publish-updater-receipt.sh <TAG> <RECEIPT>` before the RC
  is called operationally complete.
<!-- release-check-id: RC-MANUAL-007 -->
- [ ] Provider-owned CLI live smoke is recorded for each preferred headless CLI
  surface using the privacy-safe checklist in
  `docs/qa/provider-cli-compatibility-matrix.md`
<!-- release-check-id: RC-MANUAL-008 -->
- [ ] E19 desktop smoke release-decision manifest is generated and accepted
  before final sign-off; it must include History-First evidence mapping for
  every release-critical claim and must reject missing, stale, incomplete, or
  privacy-blocked evidence. When `exact_sha_ci_substitute` is selected, the
  manifest must bind automatic checks, all four Release Smoke rows, and
  Integrity Gates to the exact public commit while keeping macOS TCC
  grant/revoke behavior and consent-byte invariance `deferred_unproven`.

## Test Layers Verification
<!-- release-check-id: RC-TEST-001 -->
- [ ] Layer 1 (Rust): `cargo test --workspace` — 0 failures
<!-- release-check-id: RC-TEST-002 -->
- [ ] Layer 2 (Mock IPC): `pnpm test` — 0 failures
<!-- release-check-id: RC-TEST-003 -->
- [ ] Layer 3 (Playwright): `pnpm test:e2e` — 0 failures
<!-- release-check-id: RC-TEST-004 -->
- [ ] Layer 4 (Tauri WDIO): `run-e2e-tauri.sh` — 0 failures

## Parent/Public Source Boundary
<!-- release-check-id: RC-SOURCE-001 -->
- [ ] Release-prep commit was created from `clients/maekon-client` in parent
<!-- release-check-id: RC-SOURCE-002 -->
- [ ] Parent repository PR for the release/export change is merged before the public export PR is marked ready or merged
<!-- release-check-id: RC-SOURCE-003 -->
- [ ] Internal export dry-run passed from the parent source tree: `clients/maekon-client/scripts/export-public-repo.sh --dry-run --worktree`
<!-- release-check-id: RC-SOURCE-004 -->
- [ ] Public export was generated from the merged parent source SHA, not from an unmerged local-only branch
<!-- release-check-id: RC-SOURCE-005 -->
- [ ] Public export provenance records the parent SSOT source SHA, client
  subtree SHA, generated export snapshot SHA-256, public repository target SHA,
  public content diff result, and `source_binding` status
<!-- release-check-id: RC-SOURCE-006 -->
- [ ] Unsigned public export provenance was treated as advisory only; source
  SHA trust came from a verified Ed25519 `source_binding.signature` or GitHub
  artifact attestation whose protected workflow recomputed the client subtree
  and exported snapshot from exact immutable commits
<!-- release-check-id: RC-SOURCE-007 -->
- [ ] Public export PR was opened by the Maekon GitHub App/bot via `public-export-pr.yml` or `update-public-repo-clone.sh --open-pr`, not by the maintainer user who must approve it
<!-- release-check-id: RC-SOURCE-008 -->
- [ ] Any early public PR used for CI preview stayed draft until the merged parent source SHA was re-exported or confirmed to produce no public diff
<!-- release-check-id: RC-SOURCE-009 -->
- [ ] Public export PR merged into `pseudotop/maekon-client`
<!-- release-check-id: RC-SOURCE-010 -->
- [ ] Public pull-request CI alone was not used as cross-platform build proof; for runtime/build-impacting exports, the public branch has a successful manual `CI` `workflow_dispatch` run
<!-- release-check-id: RC-SOURCE-011 -->
- [ ] Public `main` branch protection or an active ruleset is enabled before tagging
<!-- release-check-id: RC-SOURCE-012 -->
- [ ] Public tag is an annotated signed tag and GitHub reports tag signature verification as passing
<!-- release-check-id: RC-SOURCE-013 -->
- [ ] Public tag points at the reviewed public repository state
<!-- release-check-id: RC-SOURCE-014 -->
- [ ] Parent source SHA and public export SHA are recorded in the release notes

## Artifact Integrity
<!-- release-check-id: RC-ARTIFACT-001 -->
- [ ] GitHub Release exists under `pseudotop/maekon-client`
<!-- release-check-id: RC-ARTIFACT-002 -->
- [ ] Every downloadable artifact has a matching `.sha256` sidecar
<!-- release-check-id: RC-ARTIFACT-003 -->
- [ ] Release bundle contains `sbom.cdx.json` and `sbom.cdx.json.sha256`; the
  checksum verifies against the generated SBOM
<!-- release-check-id: RC-ARTIFACT-004 -->
- [ ] Signature sidecars are present when signature verification is advertised
<!-- release-check-id: RC-ARTIFACT-005 -->
- [ ] macOS artifacts are signed, notarized, and stapled when applicable
<!-- release-check-id: RC-ARTIFACT-006 -->
- [ ] Stapling was confirmed **on the published release assets**, not on the
  notarization run's own output:
  `./scripts/verify-published-macos-notarization.sh <TAG>`. A green Release
  workflow does not imply notarization succeeded — `Notarize macOS Release
  Assets` runs separately and can fail on its own (#10935)
<!-- release-check-id: RC-ARTIFACT-007 -->
- [ ] The final notarized bytes for `maekon-macos-universal.dmg` and
  `maekon-macos-universal.pkg` were republished with regenerated `.sha256`,
  `.sig`, and provenance after stapling
<!-- release-check-id: RC-ARTIFACT-008 -->
- [ ] `notarization-final-byte-manifest.json` is present and records the same
  SHA-256 digests as the final downloadable macOS installers
<!-- release-check-id: RC-ARTIFACT-009 -->
- [ ] Release Guard accepted the current macOS release assets and would reject a
  stale checksum or stale signature sidecar
<!-- release-check-id: RC-ARTIFACT-010 -->
- [ ] GitHub artifact attestations/provenance exist for the final `dist/*`
  release subjects, and the release workflow evidence points to the current tag
  commit rather than an older export or build rerun
<!-- release-check-id: RC-ARTIFACT-011 -->
- [ ] Installer smoke uses `pseudotop/maekon-client` release URLs
<!-- release-check-id: RC-ARTIFACT-012 -->
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
<!-- release-check-id: RC-DOC-001 -->
- [ ] CHANGELOG.md contains a curated `## [<version>] - YYYY-MM-DD` section that passes `scripts/verify-release-notes-policy.sh --public --version <version>`
<!-- release-check-id: RC-DOC-002 -->
- [ ] Breaking changes documented (if any)
<!-- release-check-id: RC-DOC-003 -->
- [ ] Release notes and install docs explain that Maekon is the app display name while `maekon-*` artifacts and the `maekon` CLI command remain compatibility identifiers
<!-- release-check-id: RC-DOC-004 -->
- [ ] Final release notes distinguish the exported source state from the
  published binary release state: parent SSOT SHA, public export snapshot SHA,
  public repository commit/tag, SBOM checksum, and artifact attestation evidence
  are listed separately
<!-- release-check-id: RC-DOC-005 -->
- [ ] Release notes mention provider-owned CLI drift diagnostics: update the
  provider CLI, restart Maekon when Settings reports a stale process
  environment, refresh Support Diagnostics, and include only sanitized provider
  CLI diagnostics in bug reports
<!-- release-check-id: RC-DOC-006 -->
- [ ] If this is the first public release, README/install docs no longer say release assets are unavailable

## Sign-off
<!-- release-check-id: RC-SIGNOFF-001 -->
- [ ] Maintainer approval
<!-- release-check-id: RC-SIGNOFF-002 -->
- [ ] RC release created via `./scripts/release.sh <VERSION>` followed by `./scripts/publish-rc-tag.sh <VERSION>`.
<!-- release-check-id: RC-SIGNOFF-003 -->
- [ ] Stable release created by running `promote-stable.yml` to open a stable promotion PR, merging that PR into `main`, then running `./scripts/publish-stable-tag.sh <VERSION>` from latest `main`.
<!-- release-check-id: RC-SIGNOFF-004 -->
- [ ] **Do NOT use `git tag` directly** — the publish scripts synchronize release checks and create signed annotated tags that the release workflow verifies through GitHub.
<!-- release-check-id: RC-SIGNOFF-005 -->
- [ ] If a tag has already been pushed and the release job failed, follow `docs/guides/release-tag-recovery.md` before deleting anything. Whether any asset was published is what decides the procedure, and it must be checked first — `v0.0.1-rc.9` was recovered by replacing the tag only because the failure happened before publication.
