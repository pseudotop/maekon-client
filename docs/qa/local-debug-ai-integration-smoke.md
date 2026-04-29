# Local Debug AI / Integration Smoke Guide

Use this guide when you want to run the desktop client locally in debug mode and confirm that the current AI and integration stack is healthy enough for hands-on verification.

This is not a replacement for:

- Rust/unit/integration tests
- fake integration server compatibility tests
- upstream server contract validation

It is the fastest local path for confirming that the desktop app, web delivery layer, and runtime wiring behave correctly together.

## 1. Recommended Local Preparation

Run these first from the repository root:

```bash
./scripts/cargo-cache.sh check --workspace
./scripts/cargo-cache.sh test -p oneshim-network --test integration_fake_server -- --nocapture
./scripts/cargo-cache.sh test -p oneshim-web integration --lib -- --nocapture
./scripts/cargo-cache.sh test -p oneshim-web settings_service --lib -- --nocapture
```

If the frontend dependencies are not installed yet:

```bash
cd crates/oneshim-web/frontend && pnpm install
```

## 2. Start the App in Debug Mode

Preferred interactive desktop run:

```bash
cd src-tauri && cargo tauri dev
```

For headless or remote smoke paths where macOS tray bootstrap can fail:

```bash
ONESHIM_DISABLE_TRAY=1 ./scripts/cargo-cache.sh run -p oneshim-app -- --offline --gui
```

Use the headless variant only for non-interactive debug/smoke sessions.

## 3. What to Check Manually in the Running App

### AI / Provider Surface Smoke

- Open Settings.
- Confirm current AI sections render without handler/runtime errors.
- Change provider surface selections and verify the form updates cleanly.
- Confirm model discovery, readiness, and validation messages are coherent.
- Verify OCR and LLM settings can save and reload without state drift.

### Integration Smoke

- Open the integration-related status surface.
- Confirm auth/session status renders.
- Confirm telemetry/status fields render without null-state crashes.
- If fake or local integration input is available, confirm inbox/prompt status updates.
- Confirm no obvious runtime error loop appears in logs when the app is idle.

## 4. What Can Be GUI-Automated Reliably

These are good candidates for automated debug-mode coverage.

- Web delivery flows inside the Tauri webview
- Settings forms and surface selection flows
- Integration status/read-only telemetry panels
- Inbox list rendering and prompt state transitions
- Thin-handler to service wiring regressions

Current repo support:

- Web-only UI automation:
  - `cd crates/oneshim-web/frontend && pnpm test:e2e`
  - `cd crates/oneshim-web/frontend && pnpm test:e2e:live`
- Full Tauri desktop automation:
  - `cd crates/oneshim-web/frontend && pnpm test:e2e:tauri`

## 5. What Is Harder to Automate Even in Debug Mode

These are not impossible, but they are materially harder and usually need mocks, harnesses, or targeted smoke runs.

- Native tray behavior
- macOS dock/window visibility timing
- OS-level focus/idle/windowserver behavior
- External browser/device auth loops
- Real provider OAuth consent screens
- Real CLI subprocess auth state on a developer machine
- Network chaos and long-lived reconnect behavior against a real remote server

This is why the project keeps multiple layers:

- Rust tests for logic and boundaries
- fake integration server tests for client-side compatibility
- web/Tauri GUI automation for delivery behavior
- manual debug smoke for real desktop wiring

## 6. Practical Guidance

- For web delivery regressions, prefer automated GUI tests first.
- For Tauri runtime wiring, use `pnpm test:e2e:tauri` plus manual debug smoke.
- For AI/integration protocol behavior, trust Rust tests and fake-server coverage before manual clicking.
- For OS shell behavior and real external providers, assume a short manual smoke pass is still required.

## 7. Exit Criteria for a Good Local Debug Run

- App starts without panic or endless restart behavior.
- Settings and integration screens load cleanly.
- AI provider surface selection and persistence behave correctly.
- Integration status/telemetry reads look sane.
- No unexpected handler/runtime errors appear during basic navigation.
- Automated GUI coverage remains green for the touched delivery paths.
