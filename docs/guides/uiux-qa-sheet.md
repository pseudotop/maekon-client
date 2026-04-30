# UI/UX QA Sheet (Platform Scope)

This sheet is a general UI/UX quality gate for the ONESHIM web platform surfaces we currently operate.

## 1) Scope

- In scope:
  - Global navigation/header/search/theme/language controls
  - Core pages: Dashboard, Timeline, Reports, Focus, Replay, Automation, Updates, Settings, Privacy, Search
  - Shared components: cards, forms, tables/lists, dialogs, empty/error/loading states
- Out of scope:
  - Non-web native desktop shell rendering specifics
  - Future modules not currently shipped in `oneshim-web`

## 2) Severity

- `P0`: Blocks core task completion, causes wrong actions, or major trust break.
- `P1`: Causes repeated confusion, friction, or inefficient workflow.
- `P2`: Visual or interaction polish issues with low workflow impact.

## 3) Release Gates (Must Pass)

- `P0` No broken primary flows across top navigation routes.
- `P0` Clear loading/empty/error states for all major data panels.
- `P0` Keyboard focus visibility and accessible labels on critical controls.
- `P0` No severe mobile breakage (layout clipping, unreachable controls).
- `P1` English default language behavior remains stable; Korean optional support works.
- `P1` Theme parity (light/dark) without unreadable contrast regressions.

## 4) General UI/UX Checklist

### A. Information Architecture and Navigation

- [ ] Top-level nav labels are concise and unambiguous.
- [ ] Current location state is obvious (active nav, section title consistency).
- [ ] Secondary actions are grouped logically (not mixed with primary flow).
- [ ] Search and filter entry points are discoverable from relevant pages.

### B. Visual Hierarchy and Readability

- [ ] Page title, primary metrics, and critical actions have clear hierarchy.
- [ ] Spacing rhythm is consistent (no cramped/overly sparse sections).
- [ ] Text truncation behavior preserves meaning (tooltips/details when needed).
- [ ] Icon-only controls have supporting labels/tooltips where needed.

### C. Interaction Quality

- [ ] Hover/focus/active/disabled states are distinct and predictable.
- [ ] Buttons and links look and behave consistently across pages.
- [ ] Form validation timing is humane (not too early/no silent failure).
- [ ] Destructive actions require clear confirmation and recovery cues.

### D. Feedback States (Loading/Empty/Error)

- [ ] Loading states appear promptly and avoid flicker.
- [ ] Empty states explain why data is missing and what to do next.
- [ ] Error states are actionable and do not leak internals.
- [ ] Retry paths exist where transient failures are expected.

### E. Accessibility and Keyboard

- [ ] All interactive controls are reachable via keyboard.
- [ ] Focus indicators remain visible in both light and dark themes.
- [ ] Semantic roles/headings support assistive technologies.
- [ ] Color is not the only signal for status/priority.

### F. Localization and Internationalization

- [ ] Default language behavior follows product policy (English-first).
- [ ] Fallback strings do not unexpectedly mix languages in one view.
- [ ] Date/time/number formats follow selected locale expectations.
- [ ] Label lengths in Korean do not break layout.

### G. Responsiveness and Layout Integrity

- [ ] Header and nav remain usable from small mobile to desktop widths.
- [ ] Primary action buttons remain visible without awkward wrapping.
- [ ] Dense data blocks (tables/logs/charts) degrade gracefully on smaller viewports.
- [ ] No clipped text/icons in key components.

### H. Performance Perception

- [ ] Route transitions and major interactions feel responsive.
- [ ] Large lists/charts remain usable without noticeable UI jank.
- [ ] Expensive views preserve interaction responsiveness while data loads.

## 5) QA Execution Template

| Area | Check | Severity | Result (Pass/Partial/Fail) | Evidence | Owner | Due |
|---|---|---|---|---|---|---|
| Navigation | Active-state and label clarity | P0 |  |  |  |  |
| Feedback | Loading/empty/error completeness | P0 |  |  |  |  |
| Accessibility | Keyboard + focus visibility | P0 |  |  |  |  |
| Localization | English default + Korean support | P1 |  |  |  |  |
| Responsive | Mobile header/layout integrity | P1 |  |  |  |  |
| Performance | Interaction smoothness on dense views | P1 |  |  |  |  |

## 6) Operating Model

- Per PR: run targeted checks only for touched surfaces.
- Per release candidate: run full sheet and link evidence.
- Monthly: refresh checklist with recurring incidents and UX debt trends.
- Interactive QA execution tool: Playwright CLI (`pnpm qa:pwcli:open`, `pnpm qa:pwcli:snapshot`, `pnpm qa:pwcli:show`).
- Do not treat Playwright MCP interactions as release QA evidence.

## 7) Relationship to Replay QA

- Use this sheet for platform-wide UX baseline.
- Use `docs/guides/replay-uiux-qa-sheet.md` for replay-specialized depth checks.
- Both sheets should pass before replay-related releases.
