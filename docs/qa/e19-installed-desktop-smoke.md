# E19 Installed Desktop Smoke Runbook

This runbook defines the release-critical installed desktop smoke protocol for
E19. It covers first launch, consent/tray/overlay/notification handoffs,
privacy-gated synthetic provider invocation, and update/relaunch behavior.

The machine-readable evidence example is:

```text
docs/contracts/installed-desktop-smoke-manifest.v1.json
```

## Scope Boundary

This protocol does not replace checksum, signature, SBOM, notarization, or
provenance gates. It also does not reopen E14/E17 platform parity unless a new
release-critical installed-app gap is found.

All rows are workflow_dispatch/manual only until an approved protected desktop
runner is promoted. No arbitrary PR run may mutate a real profile, provider
configuration, or installed application location.

## Required Approval And Restore Proof

Use an approval flag before any run touches a normal application or profile
location. The preferred path is always a disposable OS user, VM snapshot,
throwaway app path, or throwaway profile.

If a normal application/profile location must be used, the evidence must include
restore proof:

- backup created before mutation
- post-run restore completed or pending-deletion count recorded
- path classes only, never raw paths
- cleanup status and counts
- manual cleanup checklist when cleanup is incomplete

## Consent, Tray, Overlay, Notification

Start from an isolated profile and prove `NotGranted/defaultClosed` before
monitoring. No screen or user-input evidence may be captured before consent.

Start, stop, and revoke require explicit operator approval. Evidence must link
visual markers to internal policy/audit state so screenshots are not the only
oracle.

The consent integrity evidence must be classified:

- `tamper_evident`: audit hash/chain/signature evidence is available.
- `not_proven_ui_only`: unsigned JSON or SQLite evidence proves UI behavior
  only. It cannot support durable consent-authenticity release-hardening claims.

After revoke, verify either no new frames or a consent-blocked diagnostic state.

Failure taxonomy:

- permission
- UI
- capture
- storage
- notification
- cleanup
- privacy-block

Evidence must not contain raw frame images, app names, window titles, local DBs,
or complete consent records.

## Installed First Launch

The first-launch smoke installs or unpacks the public release artifact into a
disposable location, then launches the app from that installed artifact.

Record:

- parent source SHA
- public export SHA
- public repository artifact URL
- release tag
- artifact checksum and signature status
- install path class
- profile path class
- app version and build metadata
- first window/onboarding visibility
- Settings availability
- privacy defaults: consent `NotGranted`, capture off, user-input collection off
- clean shutdown result
- cleanup path classes and counts

Use of normal application/profile locations requires an approval flag and
restore proof.

## Synthetic Provider Invocation

The synthetic provider smoke runs only from an installed release artifact in a
disposable profile.

Live provider calls are disabled by default. A real provider call requires
explicit operator approval and a dedicated test account. Synthetic fixtures are
allowed by default.

Record only:

- readiness class
- command/output shape
- provider surface id class
- audit/egress decision
- redacted result class
- release tag, release SHA, artifact checksum
- workflow/manual evidence id
- runner label
- cleanup result

Do not record raw prompt, raw stdout/stderr, OCR text, account/email/org id,
OAuth token, absolute path, or login screen screenshot.

Provider failure taxonomy:

- UI selection failure
- egress/privacy gate failure
- provider auth failure
- invocation timeout
- parser/schema failure

## Installed Update Smoke

The update smoke installs the previous release in a disposable location, then
updates to the candidate release using the release-supported install/update
path.

Record:

- previous release tag, SHA, version, and artifact checksum
- candidate release tag, SHA, version, and artifact checksum
- update path class
- profile path class
- provider routing/settings survival
- consent state preservation
- tray/notification state
- relaunch version/build metadata
- Settings availability
- clean shutdown
- cleanup status and path-class counts

No normal user profile, provider CLI config directory, normal application
directory, or standard app-data path may be used without approval flag and
restore proof.

Failure leaves only a privacy-safe cleanup checklist with path classes and
counts, not raw paths. The result is `hard_block` unless explicitly deferred by
the release owner as `soft_block`.
