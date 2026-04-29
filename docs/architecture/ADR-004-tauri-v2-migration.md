[English](./ADR-004-tauri-v2-migration.md) | [한국어](./ADR-004-tauri-v2-migration.ko.md)

# ADR-004: Tauri v2 Migration (iced → Tauri v2 + WebView)

**Date**: 2026-03-04
**Status**: Accepted
**Deciders**: Maekon maintainers

## Background

The `oneshim-ui` crate implemented a GUI based on iced 0.12. The following problems arose:

1. **Rendering limitations** — iced's immediate-mode renderer shows performance degradation with complex data visualizations (timelines, heatmaps).
2. **Duplicate web dashboard** — An equivalent web UI already existed via Axum + React. Maintaining two UIs increased cost.
3. **Platform inconsistency** — iced renderer behaviour differed across macOS, Windows, and Linux.

## Decision

Remove iced and use Tauri v2 to wrap the existing React web dashboard in a native desktop shell.

## Implementation

- `src-tauri/` directory: Tauri main binary entry point.
- Embed the existing `crates/oneshim-web/` React app via Tauri WebView.
- IPC: `tauri::command` macro for Rust ↔ JavaScript communication.
- System tray: `tauri::tray` API.
- Auto-update: **`self_update`** (retained). The custom updater (`src-tauri/src/updater/`) provides SHA256 + Ed25519 verification, rollback, prerelease filtering, version floor enforcement, and a coordinator state machine — capabilities that `tauri-plugin-updater` does not cover. Migration was evaluated (2026-04-01) and rejected due to feature loss risk with no clear benefit.

## Consequences

- ✅ Single UI codebase (React).
- ✅ Cross-platform consistency (WebKit/WebView2).
- ✅ Reduced dependencies by removing the `oneshim-ui` crate.
- ⚠️ Tauri IPC learning curve.
- ⚠️ WebView memory overhead (~50 MB).

## Alternatives Considered

| Alternative | Reason Rejected |
|-------------|----------------|
| Keep iced | Complex UI limitations; dual UI maintenance cost |
| Egui | Same limitations as iced |
| Electron | Excessive memory and bundle size |
