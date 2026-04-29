# Staged Rollout Convention for Release Authors

**Audience**: the maintainer cutting a new `v*` tag via `release.sh` / `promote-stable.sh`.
**Purpose**: explain how to control what percentage of installed Maekon clients receive a new release, so a regression is contained rather than reaching every user at once.

---

## TL;DR

Add exactly one HTML comment to the GitHub Release body:

```
<!-- rollout:5 -->
```

Clients pick up the percentage on their next 24-hour check cycle. Edit the release body to bump the percentage as confidence grows (5 → 25 → 50 → 100). Drop to `<!-- rollout:0 -->` to stop all further distribution.

---

## How it works

Every client maintains a stable per-install UUID, `installation_id`, generated on first launch and persisted in the user's config file. When the updater polls GitHub Releases, it:

1. Parses the release body for the first `<!-- rollout:N -->` HTML comment.
2. Hashes `installation_id + version_string` with FNV-1a.
3. Takes `hash % 100` as the bucket number.
4. If `bucket < N`, the installation is eligible; otherwise, the updater returns "up to date" and the user never sees the update.

**Determinism**: the same `(installation_id, version)` pair always maps to the same bucket. Cohort membership does not change across checks — only by bumping `N`.

Client code: `src-tauri/src/updater/mod.rs::is_eligible_for_rollout` (FNV-1a hash) + `parse_rollout_percent` (body parser).

---

## Recommended progression

| Stage | Percent | Observation window | Typical trigger to advance |
|-------|---------|---------------------|----------------------------|
| Canary | **5** | 24-48 h | No crash-rate spike, no rollback-rate spike in telemetry |
| Early | **25** | 3-5 days | User reports steady, no regressions in issue tracker |
| Broad | **50** | 3-5 days | Continued stability |
| Full | **100** | — | Release is considered safe for everyone |

You may skip stages for trivial changes (docs, packaging-only), but every release should at minimum touch Early (25%) for 24 hours before jumping to 100.

---

## Where to place the comment

Anywhere in the GitHub Release body. The updater's regex-style parser looks for the first `<!-- rollout:N -->` substring. Example body:

```markdown
## Maekon v0.4.40 — Released May 8, 2026

**Release Date:** May 8, 2026 UTC
**Since v0.4.39:** 42 commits · 7 contributors

<!-- rollout:5 -->

### Added

- Phase 4 Updater Hardening (D9 + D10 + D11). See [design doc](...).

### Fixed

- ...
```

---

## Editing after publish

GitHub Releases are editable. To advance the percentage:

1. Open the release in the GitHub UI.
2. Click **Edit release**.
3. Change `<!-- rollout:5 -->` to `<!-- rollout:25 -->` (etc.).
4. Click **Update release**.

Clients pick up the new percentage on their next poll (default 24 hours). There is no forced refresh — the updater will not call out to check more often than its configured interval.

---

## Emergency stop

Set `<!-- rollout:0 -->` in the release body. All clients that would otherwise be eligible now receive "up to date" and do not download. Combine with D11 automatic rollback (spec §4.6) for devices that already installed a bad build — the client probe self-recovers within two failed boot cycles.

---

## Behavior when the comment is absent

A release body with no `<!-- rollout:... -->` comment defaults to **100%** (full rollout). This matches the pre-Phase-4 behavior and keeps backward-compat for releases cut before this convention was introduced.

---

## Behavior on malformed values

| Body contains | Parsed percent |
|---------------|----------------|
| `<!-- rollout:5 -->`   | 5 |
| `<!-- rollout:100 -->` | 100 |
| `<!-- rollout:150 -->` | 100 (capped) |
| `<!-- rollout:abc -->` | 100 (fallback — treat as absent) |
| (no comment at all) | 100 |

---

## Interactions with the PreRelease channel

Users on `UpdateChannel::Stable` (default) see only non-prerelease tags. Users on `UpdateChannel::PreRelease` see RC tags as well. The rollout gate applies uniformly to both channels — you can stage an RC release to only 5% of opt-in PreRelease users the same way.

The Nightly channel enum variant exists in code but is not currently cut as a release flow. For now, do not rely on it.

---

## Interaction with the D9 signature trust array

Rollout gating is independent of cryptographic trust. A release is signed with the active private key and validated by any trusted key in the client-side `TRUSTED_PUBLIC_KEYS` array. The rollout gate decides whether a user **attempts** the download; the signature gate decides whether the download is **accepted** after it arrives. Both must pass.

See `docs/guides/updater-key-rotation.md` for key rotation procedures.

---

## Interaction with D11 automatic rollback

If the rollout bucket admits a user to a new version and the installed binary then fails to reach a self-healthy marker on two consecutive boots, D11 automatic rollback restores the previous-version backup. This is transparent to the release author — the client UI reports `UpdatePhase::RolledBack` and the rollback is captured in telemetry when enabled.

See the internal updater hardening design record for the full auto-rollback spec.

---

## Troubleshooting

**Q: I set rollout:5 but every colleague on the team is reporting they got the update.**

Likely cause: your team's `installation_id`s happen to cluster in the low-bucket range due to small sample size. With 5% rollout and a small team, a handful of devices may still be inside the cohort by chance. The hash is deterministic — check that each device's config file has a distinct `installation_id` UUID.

**Q: A user on PreRelease channel says they can't install an RC even though I set rollout:100.**

Check that the RC artifact's `.sig` file is present (signature verification is required when updates are enabled — see spec §2). If `.sig` is missing, the download fails the integrity check after download regardless of rollout.

**Q: I edited the rollout percentage but no new users are picking it up.**

Clients poll every 24 hours by default. Clients that already checked this release within the last 24h will not re-check until their next cycle.

---

## References

- Spec: internal updater hardening design record
- Implementation: `src-tauri/src/updater/mod.rs::is_eligible_for_rollout`, `parse_rollout_percent`
- Tests: `update_check_respects_rollout_exclusion`, `update_check_without_installation_id_is_excluded`, plus 5 unit tests on the hash + parser functions
- Adjacent docs: `docs/guides/updater-key-rotation.md` and the internal updater hardening design record
