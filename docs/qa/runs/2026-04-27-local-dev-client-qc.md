# Local Dev Client QC Run - 2026-04-27

## Scope

- App under test: `Maekon Dev.app`
- Bundle identifier: `com.oneshim.client.dev`
- Web dashboard port: `127.0.0.1:10090`
- Vite dev port present during setup: `127.0.0.1:5273`
- Method: native macOS interaction via Computer Use against the debug bundle.
- Reference benchmark: `docs/qa/screen-studio-tray-status-benchmark.md`

## Confirmed

- The release app (`com.oneshim.client`) was not used for this pass. The running app was `Maekon Dev` / `com.oneshim.client.dev`.
- The compact tracking panel renders and exposes the main state (`Capturing` / `Paused`) with pause/resume, collapse/expand, dashboard, manual capture, scene analysis, AI suggestions, focus mode, and settings entries.
- Pause/resume changes state immediately and updates the control label.
- First-run permission setup distinguishes enabled and attention-needed states. `Accessibility` showed enabled; `Screen Capture` and `Notifications` showed attention needed, and the required `Next` action stayed disabled.
- Dashboard eventually reaches `Realtime` / `Connected` after an initial connecting state.
- Overview, System Metrics, Activity Heatmap, Day View, Timeline, Replay, Settings, and Privacy routes rendered without crash.
- Manual capture saved a new screenshot: privacy counters increased from 2 to 3 screenshots.
- Settings show web dashboard port `10090` and external access disabled.
- AI provider routing is local by default; external data policy is strict; unredacted OCR upload and scene action execution are disabled.
- Privacy data controls surface stored counts, local-only copy, masking copy, date-range deletion, and export/restore entry points.

## Findings

1. `cargo tauri dev` can lead QC toward the installed release app identity. Use the built debug bundle (`target/debug/bundle/macos/Maekon Dev.app`, bundle id `com.oneshim.client.dev`) for native QC to avoid release/dev confusion.
2. The tracking panel says `Offline - local capture + analysis available` even while the dashboard footer is `Connected`. This may be technically accurate for server/LLM/CLI lanes, but it reads like an app-wide contradiction.
3. Manual Capture has no immediate success/failure feedback in the tracking panel. The capture was persisted, but the user must open the dashboard/privacy counters to know it worked.
4. The Updates card can show "Already on latest version" while still showing stale/blocked approval copy. That creates a confusing "latest but blocked" state.
5. The Privacy `Consent` tab currently contains `Delete All Data` rather than consent controls. The label and destructive content do not match user expectations.
6. Replay playback speed is inconsistent with the `1x` label. In this run, after `Skip to Start` and `Play`, about 5 seconds of waiting advanced the replay clock from `09:00:00` to `09:01:29`. Source code in `usePlaybackState` suggests `1x` should advance roughly one second per second, so this needs deeper reproduction.
7. Timeline grid capture items expose unlabeled buttons in the accessibility tree. List view improves this with labels like app, importance, and timestamp.
8. Locale presentation is mixed in several places when the app language is English: English labels with Korean-formatted dates or fallback fragments. This matches an existing replay QA backlog item, but it remains visible in the current app.
9. Playbooks currently behave as a catalog surface, not an execution surface. The `/playbooks` route lists built-in coaching templates and automation preset summaries, but it does not expose a run/apply/preview affordance from the cards. Coaching templates are used indirectly by the coaching engine, and automation presets are executable from the Automation page, but this library page itself has not been exercised as an actionable workflow.
10. Automation statistics used success/error/warning colors even for all-zero idle metrics, making an inactive automation dashboard look busier and more alarming than its state warranted.
11. Execution Policies did not give enough first-policy guidance. The empty state and create form surfaced fields, but not the practical starting shape: one trusted process, confirmation enabled, then review execution history.
12. The Update Channel UI exposed `nightly` as a normal selectable channel even though nightly artifacts are not supported for this build stream yet.
13. Settings > Focus Auto could crash into the route error boundary when persisted settings were missing the newer `focus_auto` object.

## 2026-04-28 Follow-Up

- Debug bundle guardrail was added to `scripts/build-macos-dev-bundle.sh`; it now verifies `com.oneshim.client.dev` and `Maekon Dev` before reporting the launch command.
- Tracking panel degraded-state copy now says `Local mode` instead of a broad `Offline` status, reducing conflict with the dashboard footer.
- Bottom status bar degraded-state copy also now says `Local mode` so it no longer implies that the whole app is offline when only the realtime stream is disconnected.
- Tracking panel manual-capture feedback is now live-region based and stays visible longer. Native Computer Use clicks focus the control reliably, but the visual feedback path still benefits from manual mouse verification because the capture action can complete between snapshots.
- Updates status no longer shows the stale approval-blocked warning for non-actionable polling-stale/idle states.
- Privacy destructive controls are labeled as `Danger Zone` instead of `Consent`.
- Timeline grid thumbnails now expose descriptive button labels with app, window, importance, timestamp, and selection state.
- Replay playback now derives position from monotonic elapsed time instead of timer callback count or system wall clock. Unit coverage includes timeline refetches, over-eager timer callbacks, and forward system-clock jumps.
- Tray menu degraded-state copy now uses `Local mode`, service rows use readable `connected` / `unavailable` labels, the dashboard action is labeled `Show/Hide Dashboard`, and update actions are disabled unless an update is actually pending or ready to install.
- Update deferral is ignored by the coordinator when the current update phase is not actionable, preventing stale tray/API actions from creating a misleading deferred state.
- System metrics chart tooltips and status-bar memory now use user-facing rounded values such as `10.6%`, `12.7GB`, and `8.0GB` instead of raw floating-point or large MB values.
- Activity report chart tooltips now keep metric names visible: daily bars show `Events` and `Captures`, while hourly activity is labeled as `Events + captures`.
- Export Data system metrics tooltips now keep `CPU` and `Memory` labels visible, and the chart lines use the same colors as their legend swatches.
- Automation idle metrics now stay visually neutral unless a count/rate actually indicates success, failure, denial, timeout, or blocked work.
- Execution Policies now use a guided first-policy empty state and a policy preview beside the create form, so the first useful configuration is visible before saving.
- Nightly update channel selection is disabled in the Updates page and Settings update selector; if an existing setting is still `nightly`, the Updates page asks the user to switch to a supported channel instead of showing it as active.
- Focus Auto now normalizes missing legacy `focus_auto` settings to safe defaults before rendering, so the tab opens instead of falling into the route error boundary.
- Computer Use is suitable for confirming the native route/UI state, but precise stopwatch-style replay timing across separate agent tool calls is noisy because the app keeps playing while the agent/tool pipeline is between calls. Use the replay hook regression tests as the precise timing evidence, and Computer Use as the native smoke check.

## Guardrails

- Did not grant macOS Screen Recording or Notification permissions.
- Did not click destructive privacy actions (`Delete Selected Range`, `Delete All Data`).
- Did not trigger AI suggestions or scene actions that could plausibly require an external provider or sensitive screen-context handling.

## Remaining Follow-Up Candidates

- Visually re-check the tray menu in the native debug bundle after the next build, including disabled update actions when no update is actionable.
- Manually verify tracking-panel capture feedback with a mouse-driven native run, because agent snapshots can miss fast success/failure transitions.
- Decide whether the tray should add `Capture Now`, permission status, or diagnostics/log entries from the Screen Studio benchmark.
- Continue locale cleanup for English UI paired with Korean-formatted dates or fallback fragments.
- Add Playbooks QA coverage beyond empty states: populated coaching/preset libraries, filters, the relationship between Playbooks and Automation preset execution, and whether the page should offer explicit `Run`, `Open in Automation`, or preview affordances.

## Debug Bundle Guardrail

For native macOS QC, build and launch the debug bundle directly:

```bash
./scripts/build-macos-dev-bundle.sh
open -n "target/debug/bundle/macos/Maekon Dev.app"
```

Before recording results, confirm:

- App name is `Maekon Dev`.
- Bundle identifier is `com.oneshim.client.dev`.
- Any installed release `Maekon` process has been quit first.
- `cargo tauri dev` is not used as the evidence source for native app identity checks.
