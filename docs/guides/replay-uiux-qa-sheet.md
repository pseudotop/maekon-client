# Replay UI/UX QA Sheet

This checklist is for replay-first products where timeline trustworthiness, event alignment, and investigation speed matter more than generic dashboard polish.

## 1) Scope

- Product surface: `oneshim-web` replay flow (`/replay`) plus timeline navigation handoff.
- Primary user jobs:
  - Reconstruct what happened in a work session quickly.
  - Identify critical transitions (app switches, idle blocks, high-importance frames).
  - Trust that replay data is complete, or clearly informed when it is not.

## 2) Severity Model

- `P0`: Breaks trust or core investigation workflow. Must fix before release.
- `P1`: Significantly slows investigation or causes repeated confusion.
- `P2`: Polish and consistency issues that do not block investigation.

## 3) Release Quality Gates (Must Pass)

- `P0` Replay can open and scrub a session without crash, freeze, or blank dead-end UI.
- `P0` Event-frame temporal drift is visibly consistent (target <= 1s perceived mismatch in normal playback).
- `P0` Missing frame/image states are explicit and actionable (no silent blank panel).
- `P0` Keyboard-only operation covers play/pause, seek, and item focus path.
- `P0` No critical accessibility failures on replay page (dialog semantics, focus, readable labels).
- `P1` 30-minute session with dense timeline remains interactive (no major jank while scrubbing).
- `P1` Language consistency: default English, Korean optional, no mixed fallback artifacts.

## 4) Replay-Centric QA Checklist

### A. Timeline/Scrubber Fidelity

- [ ] Current time indicator is stable and never "jumps backward" unexpectedly.
- [ ] App segment boundaries match event transitions.
- [ ] Idle segments are distinguishable from active segments.
- [ ] Start/end jump controls land exactly at session boundaries.
- [ ] Drag seek precision remains usable at high zoom density.
- [ ] Touch seek behavior works on mobile/tablet.

### B. Event-to-Frame Synchronization

- [ ] Selected event and displayed frame represent the same moment window.
- [ ] Active event auto-scroll keeps context visible without disorienting motion.
- [ ] Clicking event log reliably seeks replay to expected timestamp.
- [ ] High-importance moments are discoverable from both timeline and event log.

### C. Loading, Failure, and Data Integrity UX

- [ ] Initial loading state appears quickly and does not flicker.
- [ ] Empty-state copy explains why there is no replay data.
- [ ] Image load failure explains retention/path issue and next action.
- [ ] API failure state preserves user context (date range, play position where applicable).
- [ ] Partial data (missing frames but present events) is explicitly disclosed.

### D. Investigation Workflow Efficiency

- [ ] Replay header shows clear scope (session time, event/frame counts, app count).
- [ ] Date range change is predictable and does not reset unexpectedly.
- [ ] Analysts can pivot from replay to related pages (timeline/search/reports) without losing context.
- [ ] Speed controls are discoverable and include low/high speeds.

### E. Accessibility and Keyboard

- [ ] All controls have accessible names and visible focus states.
- [ ] Keyboard-only path exists for play/pause, seeking, speed change, and log navigation.
- [ ] Color is not the only signal for status/importance.
- [ ] Dialog/overlay interactions close on Escape and restore focus correctly.

### F. Localization and Copy Quality

- [ ] Default language is English when no prior preference exists.
- [ ] Korean is fully available when selected.
- [ ] Fallback strings do not force Korean while UI is in English.
- [ ] Date/time formatting follows selected locale expectations.

### G. Performance at Scale

- [ ] Long event lists remain responsive (virtualization or equivalent strategy).
- [ ] Scrubbing remains smooth with dense frame sequences.
- [ ] No memory growth trend during 5+ minute continuous playback.

### H. Privacy and Safety Signals

- [ ] Replay view clearly indicates when data may be redacted/masked.
- [ ] Sensitive contexts are not accidentally exposed via fallback paths.
- [ ] Error messages do not leak internal file paths or secrets.

## 5) QA Execution Template

Use this table for each release candidate.

| Area | Check | Severity | Result (Pass/Partial/Fail) | Evidence | Owner | Due |
|---|---|---|---|---|---|---|
| Timeline | Seek precision under dense segments | P0 |  |  |  |  |
| Sync | Event-frame alignment | P0 |  |  |  |  |
| Failure UX | Missing image fallback clarity | P0 |  |  |  |  |
| Accessibility | Keyboard replay controls | P0 |  |  |  |  |
| Localization | English default + clean fallback | P1 |  |  |  |  |
| Performance | 30-min session responsiveness | P1 |  |  |  |  |

## 6) Current Baseline (2026-02-23)

Evidence-based initial audit of current implementation:

| Area | Finding | Status | Evidence |
|---|---|---|---|
| Failure UX | Image-load failure has explicit fallback message | Pass | `crates/oneshim-web/frontend/src/pages/SessionReplay.tsx` |
| Scrubber | Mouse click/drag seek exists, idle striping and segment colors exist | Pass | `crates/oneshim-web/frontend/src/components/TimelineScrubber.tsx` |
| Localization | Replay UI includes Korean fallback literals and fixed `ko-KR` datetime formatting | Partial | `crates/oneshim-web/frontend/src/pages/SessionReplay.tsx` |
| Accessibility/Keyboard | Global shortcuts improved, but replay-specific keyboard controls are limited | Partial | `crates/oneshim-web/frontend/src/pages/SessionReplay.tsx`, `crates/oneshim-web/frontend/src/components/TimelineScrubber.tsx` |
| Mobile input | No touch handlers on scrubber | Fail | `crates/oneshim-web/frontend/src/components/TimelineScrubber.tsx` |
| Scale readiness | Event log is full render list (no virtualization/windowing) | Partial | `crates/oneshim-web/frontend/src/components/EventLog.tsx` |

## 7) Priority Fix Backlog (Replay-First)

1. `P0` Add replay-specific keyboard controls (seek step, play/pause, next/prev meaningful event).
2. `P0` Add touch scrubbing support and verify mobile seek precision.
3. `P1` Localize replay fallbacks to English-first defaults and locale-aware date formatting.
4. `P1` Add large-session performance guardrails for event log rendering.
5. `P1` Add explicit partial-data banner when frames/events coverage is incomplete.

## 8) Recommended QA Cadence

- Per PR: quick replay smoke (`P0` checks only).
- Per release candidate: full checklist with evidence links.
- Monthly: comparative UX benchmark review against replay-focused tools and update this sheet.
- Interactive replay QA runs must be performed via Playwright CLI (`pnpm qa:pwcli:open`, `pnpm qa:pwcli:snapshot`, `pnpm qa:pwcli:show`).
