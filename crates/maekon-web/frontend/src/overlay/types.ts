export type OverlayMode = 'minimal' | 'rich' | 'adaptive'
export type DismissAction = 'ok' | 'later' | 'timeout'

export interface CoachingPayload {
  message_id: string
  profile: string
  trigger_type: string
  text: string
  auto_dismiss_secs: number
  explanation: string
}

export interface UpgradePayload {
  message_id: string
  personalized_text: string
}

export interface FocusHighlightTarget {
  candidate_id: string
  x: number
  y: number
  width: number
  height: number
  color: string
  label: string | null
}

export interface FocusHighlightPayload {
  handle_id: string
  targets: FocusHighlightTarget[]
}

export interface GoalProgressItem {
  regime_label: string
  current_minutes: number
  target_minutes: number
  percentage: number
  display_color: string
}

export interface GoalPayload {
  goals: GoalProgressItem[]
}

export interface CaptureStatePayload {
  paused: boolean
  indicator_visible: boolean
  consent_granted: boolean
  permitted: boolean
}

export interface PointerContextPayload {
  enabled: boolean
  x: number | null
  y: number | null
  click_count: number
  click_pulse: boolean
  reduced_motion: boolean
  ttl_ms: number
  sample_rate_hz: number
}

export interface ModePayload {
  mode: OverlayMode
}

export interface FocusModePayload {
  active: boolean
  auto?: boolean
}

/**
 * Label-only one-click automation affordance for a BOUND, pending suggestion
 * (T4.1 #7917, ADR-027). Present only on live pending items whose (type, source)
 * derives a runnable preset and whose automation is enabled. Carries no preset
 * id — `run_suggestion_action` re-derives it server-side.
 */
export interface SuggestionActionDto {
  label: string
}

export interface SuggestionViewDto {
  id: string
  title: string
  body: string
  priority: string
  category: string | null
  source: string
  confidence_score: number
  created_at: string
  reasoning: string | null
  context_scope?: {
    app_name?: string | null
    window_title?: string | null
    target_id?: string | null
  } | null
  action?: SuggestionActionDto | null
}

export type SuggestionSurfacePlacement = 'adjacent-popover' | 'window-side-panel' | 'bottom-dock'

export interface SuggestionGuiAnchorPayload {
  active_app: string
  bundle_id: string
  active_process: {
    process_name: string
    bundle_id: string
    pid: number | null
    screen_id: string
    window_title: string
    window_bounds: { x: number; y: number; width: number; height: number }
  }
  display_value: string
  expression: string
  target_entity: {
    entity_id: string
    kind: string
    label: string
    value: string
    bounds: { x: number; y: number; width: number; height: number }
    center: { x: number; y: number }
    confidence: number
    sources: string[]
  }
  highlight: {
    target_entity_id: string
    bounds: { x: number; y: number; width: number; height: number }
  }
}

export interface SuggestionSurfacePayload {
  placement: SuggestionSurfacePlacement
  anchor: SuggestionGuiAnchorPayload | null
}

export interface DetectionElementPayload {
  element_id: string
  x: number
  y: number
  width: number
  height: number
  label: string
  role: string | null
  confidence: number
  source: string
}

export interface DetectionScenePayload {
  scene_id: string
  app_name: string | null
  screen_width: number
  screen_height: number
  element_count: number
  elements: DetectionElementPayload[]
}

export interface PendingConfirmationDto {
  command_id: string
  nonce: string
  process_name: string
  args: string[]
  audit_level: string
  requested_at: string
}

/**
 * A pending Codex `app-server` approval request (E21 #5044). Emitted by the Rust
 * `CodexUiApprovalHook` as the `codex:approval-request` event payload. camelCase
 * mirrors the Rust `UiApprovalContext` `#[serde(rename_all = "camelCase")]`.
 * Carries only render-safe summary fields (no file contents / secrets). The
 * eventual answer is sent back via the `respond_codex_approval` command.
 */
export interface CodexApprovalDto {
  requestId: number
  summary: string
  /** 'command' | 'file_change' */
  kind: string
  processName: string | null
  args: string[]
  diffLineCount: number | null
  networkHost: string | null
}

export interface SuggestionHistoryDto extends SuggestionViewDto {
  feedback: string | null
}

export interface ToastItem {
  id: string
  message: string
  type: 'success' | 'error' | 'info'
  duration?: number
}

/**
 * Fullscreen overlay-policy decision pushed from Rust via the
 * `overlay:fullscreen-policy` event (magic_overlay). Rust enforces the policy
 * itself (it hides the overlay window when `overlay_allowed` is false); this
 * payload lets the frontend mirror the decision in state for UI/diagnostics.
 * Field names are snake_case to match the serialized Rust payload
 * (`OverlayFullscreenPolicyPayload` has no serde rename).
 */
export interface OverlayFullscreenPolicyPayload {
  fullscreen_detected: boolean
  policy: string
  overlay_allowed: boolean
  reason: string
}

export interface OverlayState {
  mode: OverlayMode
  coaching: CoachingPayload | null
  coachingQueue: CoachingPayload[]
  focusHighlight: FocusHighlightPayload | null
  focusMode: boolean
  focusModeAuto: boolean
  goals: GoalProgressItem[]
  captureState: CaptureStatePayload
  suggestionsPanelOpen: boolean
  suggestions: SuggestionViewDto[]
  suggestionBadgeCount: number
  suggestionSurface: SuggestionSurfacePayload
  captureFlashTimestamp: string | null
  pointerContext: PointerContextPayload | null
  detectionScene: DetectionScenePayload | null
  detectionSelectedId: string | null
  pendingConfirmation: PendingConfirmationDto | null
  /** A pending Codex app-server approval awaiting an accept/decline/cancel (E21 #5044). */
  pendingCodexApproval: CodexApprovalDto | null
  /** Latest fullscreen overlay-policy decision from Rust; null until the first emit. */
  fullscreenPolicy: OverlayFullscreenPolicyPayload | null
}
