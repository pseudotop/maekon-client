/**
 * Sanctioned test-import barrel (issue #7721 G4b).
 *
 * An out-of-tree QC test suite (maintained outside this repository; see the
 * "Private QC bundle" section of the contributor guide) resolves `@src` to
 * this frontend's `src/` and currently imports 147 symbols across it via
 * deep paths (e.g. `@src/__tests__/helpers/render-helpers`,
 * `@src/hooks/useSSE`, `@src/api/client`). A rename of any of those paths
 * silently strands that suite, because its tests live outside this tree and
 * are invisible to a normal `rg`/refactor pass here.
 *
 * This barrel re-exports the highest-traffic symbols at one path
 * (`@src/test-api`) so a future incremental migration of the private suite
 * can depend on a single, intentionally-curated surface instead of scattered
 * deep imports. It is a pure addition — no existing deep import is rewired
 * by this change; that migration is tracked separately and happens
 * incrementally.
 *
 * Scope note: most `@src/api/client` usage in the private suite is
 * `vi.mock('@src/api/client', factory)`, which mocks by module specifier as
 * imported by the component under test — a re-export barrel cannot
 * intercept that path, so only the subset of `api/client` symbols the
 * private suite imports directly (not mock-only) is re-exported below.
 */

// Render helper — 45 deep-import call sites in the private suite (by far the
// highest-traffic symbol).
export { renderWithProviders } from './__tests__/helpers/render-helpers'

// api/client types — only the types the private suite imports directly today
// (see scope note above for why `vi.mock`-only usage is excluded).
export type {
  AppSegment,
  AppUsage,
  FocusMetrics,
  FocusMetricsResponse,
  HeatmapResponse,
  HourlyMetrics,
  ProcessSnapshot,
  SuggestionDto,
  Tag,
  TimelineItem,
  UpdateStatus,
} from './api/client'

// api/client functions — only the symbols the private suite imports directly
// today (not the `vi.mock('@src/api/client', ...)` factory call sites, which
// this barrel cannot help with; see scope note above).
export {
  createTag,
  fetchAutomationStatus,
  fetchFocusMetrics,
  fetchFrames,
  fetchHeatmap,
  fetchInterruptions,
  fetchReport,
  fetchSettings,
  fetchStorageStats,
  fetchSummary,
  fetchTags,
  fetchTimeline,
  fetchUnifiedSuggestions,
  fetchUpdateStatus,
  fetchWorkSessions,
  postUpdateAction,
  submitUnifiedSuggestionFeedback,
} from './api/client'

// SSE hook + its event/status types — ~20 deep-import call sites.
export {
  type ConnectionStatus,
  type FrameUpdate,
  type IdleUpdate,
  type MetricsUpdate,
  type RealtimeEvent,
  useSSE,
} from './hooks/useSSE'
