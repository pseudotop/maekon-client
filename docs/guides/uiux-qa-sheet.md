# Client QC Sheet (UI, Runtime, and Release Gates)

This sheet is the canonical client quality gate for the current Maekon desktop
client. It covers the `maekon-web` frontend, Tauri desktop runtime, local API
contracts, privacy/sandbox policy surfaces, and release/export guardrails.

## 1) Scope

- In scope:
  - App shell: title bar, activity bar, side panel, command palette, status bar,
    shortcuts help, toast system, route error recovery, and skip navigation.
  - First-run and dashboard flows: onboarding, dashboard overview/monitoring/
    insights, day view, and week view.
  - Monitor routes: timeline all/filters, replay timeline/events, focus score/
    sessions/interruptions.
  - Insights routes: reports activity/focus/export, coaching goals/history,
    chat, playbooks, and text/tag/keyword/semantic search capability states.
  - Manage routes: automation policies/commands/history, recalibration segments/
    overrides, execution policies, GUI point-and-confirm automation, audit
    summary/entries, and updates status/channel.
  - Bottom routes: all Settings tabs (General, Telemetry, Monitoring, Coaching,
    Audio, AI Automation, Data Storage, Sync, Focus Auto, Advanced, Tracking
    Schedule) and Privacy data/egress ledger/memory claims/consent/export.
  - Desktop-only surfaces: overlay suggestions, pointer context highlight,
    GUI candidate highlights, capture border, tracking panel, tray-adjacent
    controls, and runtime smoke handoffs.
  - Automated gates: Rust workspace checks, frontend lint/test/build/e2e,
    route/contract/OpenAPI checks, resource-budget and supply-chain checks,
    release/export checks, and Windows debug smoke.
- Out of scope for a local full-client QC run unless explicitly provisioned:
  - Live cloud/provider account calls and raw account identifiers.
  - Destructive real-input automation against the operator's real desktop.
  - Platform-specific hardware cases such as reboot, battery-state transitions,
    multi-monitor physical layouts, notarization, and store/distribution review.
  - Publishing tags, public releases, or stable promotion.

## 2) Severity

- `P0`: Blocks core task completion, bypasses privacy/policy/audit guarantees,
  causes data loss, or breaks release-critical build/runtime gates.
- `P1`: Causes repeated confusion, unreliable evidence, degraded trust, or
  inefficient workflow on a supported path.
- `P2`: Visual, copy, or interaction polish issue with low workflow impact.

## 3) Release Gates (Must Pass)

- `P0` Every route declared in `routeTree` is reachable through ActivityBar,
  SidePanel, direct URL, and CommandPalette without blank-screen fallback.
- `P0` First-run, privacy, consent, settings save/revert, and route recovery
  preserve user intent and do not lose unsaved settings unexpectedly.
- `P0` Automation, GUI actions, suggestion-derived action bindings, external
  egress, AI provider routing, and chat attachments remain fail-closed unless
  policy, consent, sandbox, ticket, and audit gates explicitly allow the action.
- `P0` Capture exclusions are enforced before capture and are visible as
  non-egress `capture_blocked` evidence in privacy/audit surfaces.
- `P0` Egress ledger and memory-claims surfaces are read-only except for
  reviewed claim retraction; they must never expose captured content, tokens,
  raw prompts, or sensitive local paths.
- `P0` Error, loading, empty, and partial-data states are actionable and do not
  leak raw sensitive values, local paths, tokens, prompts, or provider accounts.
- `P0` Desktop runtime smoke verifies sidecar build/placement, app launch, local
  web readiness, consent-closed defaults, and clean shutdown on the target OS.
- `P0` Rust and frontend build/test gates pass for the touched scope; release or
  export-impacting changes also pass the release/export guardrails.
- `P1` Overlay, tracking panel, pointer/capture indicators, and reduced-motion
  variants are visible, non-obstructive, keyboard-safe, and privacy-safe.
- `P1` English default language behavior remains stable; Korean and other
  configured locales do not break layout or mix fallback text unexpectedly.
- `P1` Light/dark theme parity and responsive layouts remain readable from
  compact mobile widths through desktop.

## 4) Product Surface Coverage

Use this map when deciding whether a full QC run has covered the current client.

| Group | Required surfaces |
|---|---|
| App shell | TitleBar, ActivityBar groups, SidePanel tree, CommandPalette deep links, StatusBar, shortcuts help, route error boundary, toast container |
| First run | Onboarding intro, OS permission copy, consent disclosure, feature overview, coaching opt-in, ready state |
| Monitor | Dashboard overview/monitoring/insights, Day, Week, Timeline all/filters, Replay timeline/events, Focus score/sessions/interruptions |
| Insights | Reports activity/focus/export, Coaching goals/history, Chat, Playbooks, Search text/tag/keyword/semantic states |
| Manage | Automation policies/commands/history, GUI point-and-confirm flow, Recalibration segments/overrides, Execution Policies, Audit summary/entries, Updates status/channel |
| Settings | General, Telemetry, Monitoring, Coaching, Audio, AI Automation, Data Storage, Sync, Focus Auto, Advanced, Tracking Schedule |
| Privacy | Data controls, egress ledger, memory claims/retraction, consent/danger zone, export/backup/restore, audio disclosure, erasure and delete-all flows |
| Desktop runtime | Overlay suggestions, GUI candidate highlights, pointer context highlight, capture border, tracking panel, tray/menu handoffs, resource health, debug/runtime smoke |
| Release/export | Version/config sync, route integrity, HTTP/OpenAPI contract sync, release notes policy, export guardrails, provenance/secret scans, supply-chain alert acceptance |

## 5) Domain and Aggregate Coverage

Use this table after route-level coverage. Each aggregate spans multiple crates,
ports, adapters, commands, routes, and desktop behaviors; a full QC pass should
record evidence at the aggregate boundary, not only at individual screens.
For detailed domain gates, use `docs/qa/domain-qc-matrix.md`. For granular
user-flow scenarios, use `docs/qa/customer-journey-tc-map.md`.

| Aggregate | Domain units | Required QC proof |
|---|---|---|
| App shell and delivery | `routeTree`, ActivityBar, SidePanel, CommandPalette, route error recovery, HTTP manifest, OpenAPI, app commands, theme/i18n | Every route and API/IPC entry is reachable or intentionally documented as headless-only; no blank fallback; route, command, HTTP manifest, and OpenAPI checks pass. |
| Observation and capture | Capture consent, tracking schedule, privacy pause, excluded apps/titles, frame storage, timeline, replay, dashboard, blocked-capture evidence | Consent, schedule, pause, and exclusion precedence is clear; excluded apps are blocked before capture; frame/timeline/replay/dashboard views do not reveal hidden or excluded data. |
| Privacy, consent, audit, and egress | Consent manager, policy gates, audit log, egress ledger, memory claims, retraction, GDPR/export/delete, provider egress | Ledgers and claims are read-only except reviewed retraction; retracted claims are excluded from later use/export; revocation is persisted before in-memory policy changes; retained ledgers are not erased by unrelated delete flows. |
| Intelligence, search, and learning | Search text/tag/keyword/semantic, semantic capability service, reports, digests, memory graph, feedback scorer store, coaching effectiveness store, regime reaction store | Search mode labels are honest; semantic search degrades when unavailable; learned feedback and regime state survive restart; learned relevance gates affect all suggestion producers; stale or partial intelligence is clearly marked. |
| Suggestions and automation bridge | Suggestion manager, overlay suggestions, derived bindings, `run_suggestion_action`, automation hint execution, feedback and dismissal | Suggestion-derived actions remain disabled until automation, privacy, sandbox, and execution policy gates pass; feedback/replay events are recorded; one-click run paths are auditable and fail closed. |
| Automation, GUI HITL, and sandbox | Automation policies, presets, execution policy persistence, GUI scan/highlight/confirm/execute/events, HMAC ticket/keychain, sandbox worker, Windows token limits | GUI automation follows scan, highlight, ticket confirmation, execution, events, and history; stale or replayed tickets fail; sandbox and unsupported Windows limits are surfaced honestly; presets are app-agnostic and non-destructive unless explicitly configured. |
| Chat, providers, audio, and secrets | AI sessions/messages, provider surface catalog, OAuth/integration, secret backend/projection, audio capture, STT/VAD/cloud key settings | Raw tokens, accounts, home paths, prompts, and attachments are sanitized; audio-disabled builds expose disabled states; VAD/cloud-key settings persist and fail closed under missing consent or provider readiness. |
| Sync, spool, and external IO | Sync engine/status/peers, LAN trust pins, batch uploader, upload spool, OpenAPI/export, external endpoints, read-only MCP context spec | Sent markers are written only after success; retries survive spool re-prime; revoked trust pins stay revoked; external IO is consent/policy/audit gated; MCP remains default-off, local, read-only, and stable-ID-only until implementation gates pass. |
| Storage, backup, migration, and retention | Storage stats, backup/restore, retention/delete range/delete all, schema migrations, frame metadata, export formats | Storage usage is understandable; backup/restore is explicit and test-profile safe; migrations preserve domain invariants; delete/export updates dependent views without erasing intentionally retained evidence. |
| Integrations, inbox, and realtime channels | Integration auth/status/audit, device auth, inbox refresh/ack/dismiss, update/app/GUI streams, external live config | Auth flows are cancellable; inbox actions are idempotent and auditable; stream reconnects do not duplicate actions; external live config remains gated as documented. |
| Runtime, resource, and release | App runtime launch wiring, sidecar placement, tray/autostart, resource usage snapshot, memory profiler, no-default-features build, release/export/supply-chain gates | Resource budgets produce actionable diagnostics; tray and optional items are cfg-gated under no-default-features; sidecars build and shut down cleanly; release/export and supply-chain evidence is current for the claim being made. |

## 6) General QC Checklist

### A. Information Architecture and Navigation

- [ ] ActivityBar group labels are concise and group membership is obvious.
- [ ] SidePanel tree selection, grouping, collapse/resize, and keyboard resize
      work across Monitor, Insights, Manage, Settings, and Privacy modes.
- [ ] CommandPalette includes every top-level route and child route from
      `routeTree`; search, keyboard selection, Escape, and focus trap work.
- [ ] Direct route URLs and default-child redirects land on the expected view.
- [ ] Current location state is visible through active nav, side-panel
      selection, page title, and child-route title.

### B. Visual Hierarchy and Layout Integrity

- [ ] Page title, primary metrics, critical actions, and secondary actions have
      clear hierarchy without marketing-style excess.
- [ ] Dense operational screens remain scan-friendly and do not nest cards
      inside cards.
- [ ] Text truncation preserves meaning, with tooltips/details when needed.
- [ ] Icon-only controls expose labels/tooltips and have stable dimensions.
- [ ] No clipped text, overlapping controls, or layout shift appears at mobile,
      tablet, desktop, and wide desktop widths.

### C. Interaction Quality

- [ ] Hover, focus, active, disabled, loading, and pending states are distinct.
- [ ] Buttons, links, toggles, sliders, selects, tabs, and dialogs behave
      consistently across pages.
- [ ] Form validation timing is humane and failed saves preserve input.
- [ ] Destructive actions require explicit confirmation and recovery cues.
- [ ] Long-running actions expose status, timeout, retry, and cancellation paths
      where applicable.

### D. Feedback States and Data Integrity

- [ ] Loading states appear promptly and avoid flicker.
- [ ] Empty states explain why data is missing and what to do next.
- [ ] Error states are actionable, retryable where appropriate, and sanitized.
- [ ] Partial-data states are explicit, especially replay, timeline, reports,
      search, egress ledger, memory claims, sync/spool, audit, provider
      readiness, MCP readiness, and update status.
- [ ] API or IPC failure does not create a blank page, infinite spinner, or
      misleading success message.

### E. Settings, Providers, and Privacy

- [ ] Settings save, floating save/revert, dirty-state banner, and route recovery
      preserve unsaved edits.
- [ ] AI Automation settings cover access mode, OCR/LLM providers, provider CLI
      readiness badges, saved profiles, sandbox, data policy, OCR validation,
      scene intelligence, and override expiry.
- [ ] Audio settings clearly distinguish local STT, cloud STT, raw audio egress,
      push-to-talk, voice activity, timeout, and privacy gate behavior.
- [ ] Privacy controls cover capture pause, excluded apps/titles, PII filter
      level, pre-capture exclusion enforcement, egress ledger filters,
      memory-claim retraction, consent withdrawal, export/backup/restore,
      delete-all, and audio disclosure.
- [ ] Provider, OAuth, sync, LAN, and external endpoint settings never display
      raw tokens, account identifiers, or local home paths in user-visible
      evidence.
- [ ] Consent, provider, automation, sync, and OAuth revocation paths persist
      the deny/revoked state before optimistic UI or in-memory policy updates.

### F. Automation, Replay, and Audit Safety

- [ ] Automation status, execution policies, commands, presets, and history show
      policy/sandbox/consent/audit state before action affordances are enabled.
- [ ] GUI point-and-confirm runs through scan, desktop highlight, candidate
      selection, HMAC ticket confirmation, execution, result, and history link
      without bypassing sandbox, focus-drift, nonce, TTL, or audit checks.
- [ ] Suggestion-to-automation bridges, including `run_suggestion_action`,
      expose derived bindings honestly and keep one-click run affordances
      disabled until automation and execution policy gates allow them.
- [ ] Automation presets such as deep-work-start remain app-agnostic,
      non-destructive by default, and honest about what will and will not run.
- [ ] Persisted automation execution policies survive restart and preserve
      disabled/deny defaults until the operator intentionally changes them.
- [ ] Intent hints and scene actions remain blocked when automation, privacy, or
      scene execution policy is disabled.
- [ ] Replay scene overlays, target locking, consent gates, and action proposals
      are understandable and tied to audit-ready evidence.
- [ ] Audit summary and entries expose enough correlation information to
      diagnose actions without leaking raw sensitive content.
- [ ] Negative paths (stale scene, focus mismatch, stale bbox, unsupported
      platform, stale or replayed ticket, sandbox denial) fail closed before
      real input is sent.

### G. Desktop Runtime and Overlay Surfaces

- [ ] Windows/macOS/Linux runtime checks use the appropriate debug or installed
      smoke path for the target platform.
- [ ] Sidecar worker build/placement matches the app target triple.
- [ ] Overlay suggestions, coaching popup, automation confirmation, pointer
      highlight, tracking border, and capture flash render on the desktop
      surface where CDP cannot observe them.
- [ ] GUI candidate highlight boxes appear only for reviewed candidates, clear
      after reset/failure, and never imply execution before confirmation.
- [ ] Windows sandbox/token status reports unsupported or unenforced limits
      honestly, including `disable_most_sids` behavior.
- [ ] Reduced-motion mode, keyboard operation, and screen-reader announcements
      remain usable for overlay and tracking panel controls.
- [ ] The fixed-height tracking panel keeps every expanded action pointer- and
      keyboard-reachable without scrolling the persistent toolbar out of view.
- [ ] Runtime overlay probes restore the real capture status, recording
      indicator, pointer layer, window visibility, and interactive layout on
      success, early return, and unwind.
- [ ] CUA-safe startup begins paused with the indicator hidden, keeps auxiliary
      WebViews uncreated until an explicit surface request, and recreates the
      tracking panel on the first Show Indicator action after startup.
- [ ] Closing or hiding an auxiliary overlay/panel destroys its idle WebView
      controller so a later explicit request exercises the lazy-create path.
- [ ] Screenshot/log evidence is bounded, redacted, and classified correctly.

### H. Accessibility, Keyboard, and Localization

- [ ] All interactive controls are reachable via keyboard.
- [ ] Focus indicators remain visible in light and dark themes.
- [ ] Dialogs, comboboxes, tree views, tabs, and live regions use correct roles
      and restore focus predictably.
- [ ] Color is not the only signal for status, priority, or severity.
- [ ] Default language is English; selected locales have complete user-facing
      strings and do not break layout.
- [ ] Locale resources contain no Unicode replacement characters or mojibake;
      integrity checks cover every supported locale, not only key parity.

### I. Performance and Responsiveness

- [ ] Route transitions, command palette search, settings tab changes, and
      dense list interactions stay responsive.
- [ ] Timeline, replay, reports, audit, and search remain usable with realistic
      data volume.
- [ ] Polling/backoff behavior does not create excessive network, IPC, or CPU
      churn.
- [ ] Resource-budget instrumentation and self CPU/RSS diagnostics report
      actionable health without creating user-visible alarm fatigue.
- [ ] Hidden or idle transparent WebViews stop compositing transient overlay
      animations; post-probe and close-to-tray CPU samples return near the
      pre-activation baseline instead of sustaining near-core saturation.
- [ ] A CUA-safe idle sample records the visible Maekon window count and a
      bounded CPU interval; only the main window remains before a surface is
      explicitly requested.
- [ ] Long-running local tasks do not freeze the UI thread.

### J. Release, Export, and Supply-Chain Guardrails

- [ ] `Cargo.toml`, `Cargo.lock`, frontend package version, Tauri metadata, and
      config sync are consistent.
- [ ] Public export include/exclude boundaries remain intentional; internal-only
      evidence stays out of public-minimal export.
- [ ] Tauri app-command capability schemas and generated permission manifests
      stay in sync with `src-tauri/build.rs` command registration.
- [ ] `maekon-app` optional desktop/tray items remain cfg-gated and pass a
      no-default-features check for release-claimed command surfaces.
- [ ] HTTP interface manifest and OpenAPI snapshots stay synchronized with
      `maekon-web` route and service changes.
- [ ] Release notes policy, public export provenance, secret scanning, and
      security-alert acceptance gates pass before release/export sign-off.
- [ ] Supply-chain gates and dependency alert resolutions are fresh enough for
      the claim being made, including RustSec audit suppressions mirrored
      between `.cargo/audit.toml` and `deny.toml` where applicable.
- [ ] Read-only MCP/local-context specs remain default-off and do not imply raw
      claim, frame, OCR, audio, action, or path access before implementation.
- [ ] Parent SSOT, exported public snapshot, and actual public repo state are
      reported separately when release readiness is discussed.

## 7) Full Client QC Command Bundle

Run the smallest safe subset for narrow PRs. For release candidates or broad
client validation, use this bundle and record exact command output summaries.

| Layer | Command |
|---|---|
| Windows pre-flight | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/diagnose-windows-dev-bootstrap.ps1` |
| Git/text hygiene | `git diff --check` |
| Config sync | `bash -lc './scripts/check-config-sync.sh'` |
| Route integrity | `bash -lc './scripts/verify-route-integrity.sh'` |
| HTTP/OpenAPI contract sync | `bash -lc './scripts/verify-web-contract-boundary.sh && ./scripts/verify-http-interface-manifest.sh && ./scripts/verify-http-openapi-sync.sh'` |
| Frontend package config | `cd crates/maekon-web/frontend && pnpm test:pnpm-config` |
| Frontend lint | `cd crates/maekon-web/frontend && pnpm lint` |
| Frontend unit tests | `cd crates/maekon-web/frontend && pnpm test` |
| Frontend production build | `cd crates/maekon-web/frontend && pnpm build` |
| Storybook build | `cd crates/maekon-web/frontend && pnpm build-storybook` |
| Browser e2e | `cd crates/maekon-web/frontend && pnpm test:e2e` |
| Rust fmt | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/run-windows-cargo-tc.ps1 -CargoArgs @('fmt','--all','--','--check')` |
| Rust check | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/run-windows-cargo-tc.ps1 -CargoArgs @('check','--workspace')` |
| Rust no-default-features | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/run-windows-cargo-tc.ps1 -CargoArgs @('check','-p','maekon-app','--no-default-features')` |
| Rust clippy | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/run-windows-cargo-tc.ps1 -CargoArgs @('clippy','--workspace','--all-targets','--','-D','warnings','-A','clippy::empty_docs','-A','clippy::derivable_impls','-A','clippy::type_complexity')` |
| Rust tests | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/run-windows-cargo-tc.ps1 -CargoArgs @('test','--workspace')` |
| Release notes policy | `bash -lc './scripts/test-release-notes-policy.sh'` |
| Release workflow governance | `bash -lc './scripts/test-release-workflow-governance.sh'` |
| Public export guardrails | `bash -lc './scripts/test-release-export-guardrails.sh'` |
| Install/signature policy | `bash -lc './scripts/test-install-signature-policy.sh'` |
| Supply-chain exemption expiry | `bash -lc 'python3 scripts/supply_chain/verify_exemption_expiry.py --deny-file deny.toml supply-chain/config.toml'` |
| Public export dry run | `bash -lc './scripts/export-public-repo.sh --dry-run --worktree <tmp-export-dir>'` |
| Windows runtime smoke | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/run-windows-debug-smoke.ps1` |

Optional or environment-gated checks:

- On Windows, run Bash scripts through `bash -lc` so Git Bash initializes
  `/usr/bin`. If `python3` is not on that Bash PATH, set `PYTHON` to a valid
  `python.exe` before release/export guardrail commands.
- `cd crates/maekon-web/frontend && pnpm test:e2e:tauri` when a real desktop
  WebDriver session is available.
- `cargo test --workspace -- --ignored` only when external services, OS
  permissions, and long-runtime prerequisites are intentionally provisioned.
- Provider-owned CLI live smoke only with configured test credentials and the
  privacy-safe checklist in `docs/qa/provider-cli-compatibility-matrix.md`.
- Integrity/supply-chain tools such as `scripts/verify-integrity.sh`,
  `./scripts/cargo-cache.sh audit`, `./scripts/cargo-cache.sh deny check`,
  and `cargo vet check` when those tools are installed and network/cache
  prerequisites are available.

## 8) QA Execution Template

| Area | Check | Severity | Result (Pass/Partial/Fail/Blocked) | Evidence | Owner | Due |
|---|---|---|---|---|---|---|
| Shell/IA | ActivityBar, SidePanel, CommandPalette, direct route coverage | P0 |  |  |  |  |
| First run | Onboarding, consent disclosure, ready state | P0 |  |  |  |  |
| Monitor | Dashboard, Day/Week, Timeline, Replay, Focus | P0 |  |  |  |  |
| Insights | Reports, Coaching, Chat, Playbooks, Search text/tag/keyword/semantic states | P1 |  |  |  |  |
| Manage | Automation, GUI point-and-confirm, Recalibration, Policies, Audit, Updates | P0 |  |  |  |  |
| Settings | Save/revert, AI Automation, Audio, Sync, Focus Auto, Tracking Schedule | P0 |  |  |  |  |
| Privacy | Consent, capture pause, egress ledger, memory claims/retraction, export/delete, audio disclosure | P0 |  |  |  |  |
| Desktop runtime | Sidecar, launch, overlay/tracking panel, GUI highlights, resource health, clean shutdown | P0 |  |  |  |  |
| Domain aggregates | App shell, capture, privacy, intelligence, automation, sync, runtime/release aggregate acceptance | P0 |  |  |  |  |
| Customer journeys | `CJ-00..CJ-05` granular scenarios from `docs/qa/customer-journey-tc-map.md` | P0 |  |  |  |  |
| Storage/integrations | Backup/restore, migration, retention, device auth, inbox decisions, realtime stream recovery | P0 |  |  |  |  |
| Accessibility | Keyboard, focus, roles, announcements | P0 |  |  |  |  |
| Localization/theme/responsive | Locale fallback, theme parity, mobile/desktop layout | P1 |  |  |  |  |
| Automated gates | Rust, frontend, route/config, HTTP/OpenAPI, release/export/supply-chain scripts | P0 |  |  |  |  |

## 9) Operating Model

- Per PR: run targeted checks for touched surfaces and any shared shell,
  privacy, automation, provider, or release guard touched by the change.
- Per release candidate: run the full command bundle, desktop runtime smoke on
  the target OS, and the product surface coverage matrix with evidence links.
- Monthly: refresh this sheet with recurring incidents, route additions, new
  settings tabs, guardrail changes, and UX debt trends.
- Interactive UI QA execution tool: Playwright CLI (`pnpm qa:pwcli:open`,
  `pnpm qa:pwcli:snapshot`, `pnpm qa:pwcli:show`).
- Do not treat Playwright MCP or ad hoc browser interactions as release QA
  evidence unless the run also records repeatable commands, screenshots, logs,
  and commit SHA.

## 10) Related QA Documents

- Use `docs/guides/replay-uiux-qa-sheet.md` for replay-specialized depth checks.
- Use `docs/qa/domain-qc-matrix.md` for domain-level gates, aggregate packs, and
  required evidence by domain.
- Use `docs/qa/customer-journey-tc-map.md` for granular customer journey
  scenarios and Computer Use TC mapping.
- Use `docs/qa/debug-client-audit-tc-runbook.md` for native desktop/runtime TCs
  that require screenshots, logs, and audit deltas.
- Use `docs/contracts/gui-session-acceptance-matrix.md` for GUI automation
  lifecycle/geometry/negative-path acceptance.
- Use `docs/release-checklist.md` and `docs/STATUS.md` for release readiness,
  public export, and mutable CI/security status.
