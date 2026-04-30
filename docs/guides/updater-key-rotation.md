# Updater Key Rotation Runbook

**Audience**: release engineering / security responders.
**Purpose**: procedure for rotating the Ed25519 signing key used by the auto-updater, with distinct flows for scheduled (low urgency) vs compromise (urgent) scenarios.

---

## Trust model recap

- **Private key** (`UPDATE_SIGNING_PRIVATE_KEY_B64` GitHub Secret): 32-byte Ed25519 seed used by the release workflow to sign each artifact (`.sig` files generated at `release.yml:1113-1149`).
- **Public keys** (`src-tauri/src/updater/trusted_keys.rs::TRUSTED_PUBLIC_KEYS`): base64-encoded 32-byte arrays compiled into every client binary.
- **Verification** (`src-tauri/src/updater/install.rs::verify_signature`): walks the trusted-keys array; accepts the signature if ANY listed key validates. Falls back to user-configured override key only when the configured value is non-empty and genuinely different from every built-in key.

Because multiple keys are trusted simultaneously, rotation happens in a **transition window** during which both old and new keys are accepted — clients updated before the window closes never experience a trust gap.

---

## Scheduled rotation (planned, low urgency)

Trigger: periodic hygiene (annual), pre-release of a stable branch, or deprecating an old key that's been in production long enough.

Window: 2 release cycles (~4-6 weeks typical).

### Step 1 — Generate new keypair

Use `scripts/rehearse-key-rotation.sh` locally to derive a fresh Ed25519 keypair. Inspect the outputs — never commit the seed. Store the seed in a password manager or similar secure vault until Step 3.

### Step 2 — Add new public key to trusted array

Create a PR that inserts the new key at **position [0]** (top) of `TRUSTED_PUBLIC_KEYS` in `src-tauri/src/updater/trusted_keys.rs`. Keep the old key at position [1]. Comment both entries with introduction date and a unique label for tracking.

Example:

```rust
pub(crate) const TRUSTED_PUBLIC_KEYS: &[&str] = &[
    // v2 — introduced 2027-03-15, scheduled rotation
    "BASE64_NEW_KEY_GOES_HERE==",
    // v1 — introduced 2026-04-18, original production key
    "GIdf7Wg4kvvvoT7jR0xwKLKna8hUR1kvowONbHbPz1E=",
];
```

Land this PR + cut a release (e.g., `v0.4.N+1-rc.1`). Clients installed from this release forward will trust BOTH keys.

### Step 3 — Switch the CI signing secret

Once the "two-key release" has been in distribution for at least **one full release cycle** (≥ 1 week with real install telemetry showing adoption), update the `UPDATE_SIGNING_PRIVATE_KEY_B64` GitHub Secret to the new seed.

Every release from this point forward signs with the **new** key. Clients on the two-key release validate these signatures via the new key (position [0]). Clients still on the **pre-rotation release** have only the old key and cannot install new-key-signed updates — they must first upgrade to the two-key release via the old-signature path, which is still valid because both keys are trusted.

### Step 4 — Remove old key

After ≥ 1 release cycle with the new key active in production, cut a PR removing the old key from the array. Example:

```rust
pub(crate) const TRUSTED_PUBLIC_KEYS: &[&str] = &[
    // v2 — introduced 2027-03-15, sole production key since 2027-04-01
    "BASE64_NEW_KEY_GOES_HERE==",
];
```

Land + release (`v0.4.N+2`). From this release forward, old-key-signed updates (if any stale ones exist) will be rejected.

### Timeline

```
[v0.4.N]    old key only    — baseline
[v0.4.N+1]  old + new keys  — transition release
[v0.4.N+2]  new key only    — post-rotation cleanup (≥ 1 cycle after N+1)
```

---

## Compromise response (urgent)

Trigger: the `UPDATE_SIGNING_PRIVATE_KEY_B64` secret is suspected or confirmed exposed (credential leak, accidental commit to public repo, ex-employee access, etc.). Old-key-signed updates are now untrustworthy, even if the old release assets were authentic at the time of publication.

### Step 1 — Immediate secret rotation

Before anything else: rotate the `UPDATE_SIGNING_PRIVATE_KEY_B64` secret at the GitHub repo level to a freshly generated Ed25519 seed. This prevents ongoing unauthorized signing — even if we suspect the leak.

### Step 2 — Derive new keypair + hotfix branch

Generate the new keypair (same tool as scheduled rotation). On a hotfix branch off `main`, **remove the compromised key entirely** (do NOT retain it) and insert the new key as the sole entry in `TRUSTED_PUBLIC_KEYS`:

```rust
pub(crate) const TRUSTED_PUBLIC_KEYS: &[&str] = &[
    // v2 — 2027-XX-YY compromise response; v1 revoked
    "BASE64_NEW_KEY_GOES_HERE==",
];
```

### Step 3 — Ship the hotfix signed with the new key

Cut `v0.4.N-hotfix` from the hotfix branch. Release workflow signs with the already-rotated secret (from Step 1). This hotfix validates on any client that has the new key compiled in.

### Step 4 — Stranded-client notification

**Users on v0.4.N-1 or earlier have ONLY the old (now-removed) key** and will reject the hotfix. They must manually re-install from a signed-by-new-key installer download. Out-of-band trust anchors vary by platform:

- **macOS**: Apple codesign signature on the DMG/PKG + (when notarization pipeline is fixed) Apple notarization. Gatekeeper validates these at install time.
- **Windows**: GitHub Release page SHA-256 hash + manual verification via PowerShell `Get-FileHash`. No Authenticode codesign currently in this project.
- **Linux**: GitHub Release SHA-256 + provenance attestation from `actions/attest-build-provenance` (`release.yml:1152-1155`). Users manually verify both the SHA-256 and the attestation.

Notify via:
- Release notes on the hotfix (explain the rotation in plain language).
- External channels: GitHub Discussions, Discord/Slack if the project has one, email list if users opted in.
- In-app: if an older client can reach the update check but rejects the signature, display a banner linking to the re-install instructions.

### Step 5 — Audit + post-mortem

- Audit all releases signed with the compromised key between the leak date and Step 1 rotation. Compare artifact checksums against a known-good source (e.g., internal build server) to detect any malicious artifacts.
- Revoke the compromised key at the GitHub repo-secret level (already done in Step 1 but double-check that the old seed is not in any backup location).
- Rotate GitHub token access for any account with repository-secret write permission.
- Document the incident in the internal security review log with timeline, scope, and remediations.

### Key differences from scheduled rotation

| Dimension | Scheduled | Compromise |
|---|---|---|
| Timing | 2 release cycle window | Immediate (hours, not weeks) |
| Old key retention | Yes (both keys resident 1-2 releases) | **No** — removed on day one |
| User impact | None (transparent rotation) | Stranded clients must re-install |
| Coordination | Engineering-led release flow | Security-led incident response |
| Notification | Release notes mention only | Release notes + external channels |

---

## Rehearsal

Run `scripts/rehearse-key-rotation.sh` **at least annually** to validate:
- Key derivation produces a 32-byte seed.
- Generated base64 public key matches `nacl.signing.SigningKey(seed).verify_key.encode()` expectation.
- Signing + verification round-trips end-to-end on the rehearsal artifacts.

Document the rehearsal outcome in a run-log (date, tester, artifact hashes) so the first real rotation isn't the first time the procedure is exercised.

---

## References

- Trust array: `src-tauri/src/updater/trusted_keys.rs`
- Verification: `src-tauri/src/updater/install.rs::verify_signature`
- Signing pipeline: `.github/workflows/release.yml:1113-1149`
- Rehearsal script: `scripts/rehearse-key-rotation.sh`
- Implementation record: internal updater hardening design and key-rotation plan
- Adjacent: `docs/guides/updater-rollout.md`, `docs/guides/updater-rollback-windows.md`
