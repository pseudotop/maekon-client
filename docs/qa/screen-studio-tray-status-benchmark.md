# Screen Studio Tray And Status UX Benchmark

Date: 2026-04-27
Source: User-provided Screen Studio screenshots from macOS recording setup.

This note captures UX reference points for Maekon/Hermes tray, status, capture,
and permission flows. The goal is not to copy Screen Studio visually, but to keep
the interaction principles visible during local QA and future product work.

## Benchmark Principles

1. Status is always visible.

   Screen Studio keeps recording mode, selected target, unavailable inputs, and
   the primary action visible in a compact floating bar. A background desktop app
   should make its current operating state legible without requiring the user to
   open the main dashboard.

2. Fast actions live in the menu bar/tray.

   The menu bar exposes direct commands such as starting a new recording,
   choosing display/window/area mode, opening settings, showing previous
   projects, and quitting. For Maekon, equivalent actions should be available
   from tray/menu controls where possible.

3. Missing permissions are not hidden.

   Screen Studio shows disabled camera, microphone, and system-audio states in
   the control bar. Maekon should surface missing screen capture, accessibility,
   automation, notification, or local-service permissions at the tray/status
   level, not only inside settings.

4. Selection mode uses dimming plus a focused target.

   Display/window/area selection uses a full-screen dim layer, a highlighted
   target, and a clear target label such as display name, resolution, and frame
   rate. For Maekon capture or automation scope selection, the user should be
   able to see exactly what is in scope before confirming.

5. Controls are icon-first, with text only where it clarifies state.

   Mode switches use familiar icons. Text is reserved for state labels, target
   metadata, and the primary action. This keeps the panel compact while staying
   readable.

6. The primary action is visually distinct.

   Screen Studio makes `Start recording` the only high-emphasis button and keeps
   secondary choices in a dropdown. Maekon should similarly make the most
   important current action obvious, for example `Resume tracking`, `Pause
   tracking`, `Open dashboard`, or `Fix permissions`.

7. Advanced choices stay nearby but out of the way.

   The gear menu provides quick toggles like hiding desktop icons, highlighting
   the recorded area, countdown, and advanced settings. Maekon should expose
   quick privacy and capture-status controls without crowding the main dashboard.

8. Preview confirms scope.

   Screen Studio shows a small preview thumbnail of the selected capture target.
   Maekon should prefer a similarly concrete confirmation when the user chooses a
   screen, window, region, or automation target.

## Maekon QA Criteria

Use this checklist when reviewing the local client:

- The tray/menu bar state communicates whether tracking is active, paused,
  scheduled, blocked by permissions, or degraded by local service failures.
- The user can pause or resume tracking from tray/menu controls without opening
  the dashboard.
- The dashboard and tray agree on capture/tracking status after changes.
- Missing permissions are visible as actionable states, not silent failures.
- Privacy mode or local-only mode is visible before data is captured or synced.
- Capture scope, if configurable, is understandable before confirmation.
- Any status panel avoids long explanatory text and favors clear labels,
  icons, and direct actions.
- Secondary controls are grouped behind menus or toggles rather than competing
  with the primary action.
- Error states include a next action such as `Open permissions`, `Retry local
  server`, or `Open logs`.
- Close-to-tray behavior is clear and does not look like the app has quit.

## Desired Tray/Menu Actions

The tray/menu should be evaluated for these user-level actions:

- Open Dashboard
- Pause Tracking / Resume Tracking
- Capture Now, if supported by the current runtime
- Toggle Privacy Mode or Local-Only Mode, if supported
- Show Permission Status
- Open Settings
- Open Logs or Diagnostics
- Quit

## Desired Compact Status Panel

A compact status surface should be able to show:

- Tracking active / tracking paused / tracking scheduled
- Capture permitted / capture blocked
- Screen permission status
- Accessibility or automation permission status
- Local dashboard/server status
- Pending sync count, when sync is enabled
- Last capture or last analysis timestamp, if useful
- One primary action for the current state

## Comparison Notes From Screen Studio

- The floating bar is horizontally compact and bottom-centered, so it feels like
  a control surface rather than a full app window.
- The app uses a dim overlay during selection, making focus and scope obvious.
- Disabled inputs are shown inline as `No camera`, `No microphone`, and `No
  system audio`; the absence of an input is treated as a state, not as hidden
  configuration.
- The menu bar duplicates major commands so the user can operate the app without
  hunting for the main window.
- Quick settings are checkmark-driven and immediately understandable.

## Non-Goals

- Do not copy Screen Studio branding, colors, layout, or exact copy.
- Do not turn Maekon into a screen recorder UI.
- Do not add decorative panels that obscure the operational state.
- Do not hide privacy or permission state inside a deep settings page.
