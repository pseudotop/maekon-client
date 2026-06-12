# Maekon Release Versioning Policy

## TL;DR

**Maekon uses SemVer at the tag/Cargo level + CalVer in user-facing surfaces** — same pattern as [`NousResearch/hermes-agent`](https://github.com/NousResearch/hermes-agent/releases) ("hybrid Option A").

```
Tag / Cargo.toml: v0.1.0-rc.3       (SemVer 2.0.0)
Release title:    Maekon v0.1.0-rc.3 — Released May 06, 2026
Release header:   **Built**: 2026-05-06 UTC · **Commit**: 6289085a9
maekon --version: maekon 0.1.0-rc.3 (build: 2026-05-06 | commit: 6289085a9)
About dialog:     same fields exposed via Tauri `get_app_build_info` IPC
```

## Why this scheme

We evaluated three classes:

| Scheme | Examples | Verdict |
|---|---|---|
| Pure SemVer | Aider, Cline, OpenHands, Goose, Continue, AutoGPT, LangChain (all AI agents); Tauri, Lapce, Spacedrive, Zed (all Tauri/Rust desktop) | Industry default — but build date invisible |
| Pure CalVer (`YYYY.MM.DD`) | yt-dlp, Black, Helix, pip, Ubuntu | Common in continuously-deployed end-user tools — rare in AI agents (0/8) and Tauri apps (0/5) |
| **Hybrid (SemVer + CalVer surfaces)** | **Hermes Agent (NousResearch)** — the *only* mainstream AI agent OSS that surfaces dates in releases | **Chosen**: build date visible everywhere users look, SemVer contract preserved |

Empirically: every comparable project (AI-agent OSS + Tauri desktop) uses SemVer at the tag level. Departing to pure CalVer would be a deliberate ecosystem departure with no payoff over the hybrid approach.

## Where build date appears

| Surface | Format | Source |
|---|---|---|
| Git tag / `Cargo.toml` | SemVer only (`v0.1.0-rc.3`) | `scripts/release.sh` |
| GitHub Release title | `Maekon v0.1.0-rc.3` | `release.yml` `RELEASE_NAME` |
| GitHub Release notes header (line 1) | `## Maekon v0.1.0-rc.3 — Released May 06, 2026` | `release.yml` "Prepend release date header" step |
| GitHub Release notes header (line 2) | `**Built**: 2026-05-06 UTC · **Commit**: 6289085a9` | same step |
| `maekon --version` (CLI) | `maekon 0.1.0-rc.3 (build: 2026-05-06 \| commit: 6289085a9)` | `src-tauri/src/main.rs` early dispatch |
| Tauri About dialog (frontend) | `version` / `build_date` / `git_sha` fields | `commands::build_info::get_app_build_info` IPC |
| Binary metadata | `BUILD_DATE` / `GIT_SHA` env vars baked at compile time | `src-tauri/build.rs` |

## SemVer contract

We follow [SemVer 2.0.0](https://semver.org/) at tag/Cargo level:

- **MAJOR** (`X.0.0`): incompatible API/UX changes — server contract breaks, settings file format breaks, IPC contract breaks
- **MINOR** (`0.X.0`): new features, backwards-compatible
- **PATCH** (`0.0.X`): bug fixes, backwards-compatible
- **Pre-release** (`-rc.N`): release candidate iteration during soak window
- Pre-1.0 (`0.x.x`): public API not yet stable — minor bumps may break compatibility (per SemVer §4)

CHANGELOG entries use [Conventional Commits](https://www.conventionalcommits.org/) parsed by `git-cliff`. Breaking changes carry `BREAKING CHANGE:` footer and are highlighted in release notes.

## Release cadence

- **RC iteration**: as needed within a release cycle (typically 1-3 RCs per stable cut)
- **RC soak window**: ~4 weeks between final RC and stable promotion (operational + external feedback)
- **Stable promotion**: via `promote-stable.yml` to create a promotion PR, followed by a maintainer-local signed stable tag after merge
- No fixed weekly/monthly cadence — driven by feature/fix readiness, not the calendar

## Protected environments

Public release automation uses three GitHub environments with required reviewers:

- `release-signing`: release App token creation, macOS signing certificates,
  update signing private key, release creation, final notarized asset
  publication, and provenance publishing.
- `macos-notarization`: Apple notarization credentials, notary logs, and
  stapled DMG/PKG final-byte artifact creation. This environment must not hold
  the update signing private key.
- `desktop-smoke`: live AI smoke and GUI/desktop smoke runners. This
  environment must not contain release signing, update signing, release App, or
  Apple notarization secrets.

Environment secrets are the expected control. Do not duplicate
`UPDATE_SIGNING_PRIVATE_KEY_B64`, `MAEKON_RELEASE_APP_PRIVATE_KEY`,
`MACOS_APP_CERT_P12_B64`, `MACOS_INSTALLER_CERT_P12_B64`, or
`MACOS_NOTARY_APP_PASSWORD` as broad repository-level secrets for desktop smoke.
Desktop smoke jobs keep read-only repository permissions and upload only
sanitized log bundles with short retention.

Admin bypass is reserved for incident recovery when required reviewers are
unavailable. Record the admin bypass reason, approver, affected run, and
follow-up rotation decision in the release notes or release-decision manifest
before publishing assets. An emergency rerun may reuse the same protected
environment only when the source SHA, release tag, and artifact checksums are
unchanged; any secret exposure suspicion requires key rotation before rerun.

## Source trust

- Release tags must be annotated signed tags; lightweight tags are not accepted.
- The release workflow verifies GitHub tag-signature verification before building artifacts.
- Public `main` must have branch protection or an active ruleset before a release tag is accepted.
- Artifact checksums, Ed25519 signatures, and provenance attestations cover the binary side of the trust chain; signed tags and branch protection cover the source side.

## Migration history

- **v0.0.1-rc.3** (2026-05-05) — first prerelease on `pseudotop/maekon-client` (post-rebrand). Three-RC iteration (rc.1 / rc.2 / rc.3) due to MSI verify guard + signed smoke fallback regressions surfaced by the new `0.0.1` line.
- This policy was codified after rc.3 publish to make the build-date surfacing pattern explicit going forward.

## References

- [Hermes Agent versioning](https://github.com/NousResearch/hermes-agent/releases) (the closest peer that surfaces dates)
- [SemVer 2.0.0](https://semver.org/)
- [calver.org](https://calver.org/) (calendar versioning standard, not adopted)
- [Conventional Commits](https://www.conventionalcommits.org/)
- Ecosystem evidence on AI-agent + Tauri-desktop versioning: see this PR's research section (CalVer adoption is rare-to-nonexistent in our category)
