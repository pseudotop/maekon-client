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
}

export interface ModePayload {
  mode: OverlayMode
}

export interface FocusModePayload {
  active: boolean
  auto?: boolean
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
  is_read: boolean
  reasoning: string | null
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

export interface SuggestionHistoryDto extends SuggestionViewDto {
  feedback: string | null
}

export interface ToastItem {
  id: string
  message: string
  type: 'success' | 'error' | 'info'
  duration?: number
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
  captureFlashTimestamp: string | null
  detectionScene: DetectionScenePayload | null
  detectionSelectedId: string | null
  pendingConfirmation: PendingConfirmationDto | null
}
