import { useEffect, useReducer } from 'react'
import { redactSuggestionViews } from '../suggestionPrivacy'
import type {
  CaptureStatePayload,
  CoachingPayload,
  CodexApprovalDto,
  DetectionScenePayload,
  FocusHighlightPayload,
  FocusModePayload,
  GoalProgressItem,
  OverlayFullscreenPolicyPayload,
  OverlayMode,
  OverlayState,
  PendingConfirmationDto,
  PointerContextPayload,
  SuggestionSurfacePayload,
  SuggestionViewDto,
} from '../types'

export type OverlayAction =
  | { type: 'show-coaching'; payload: CoachingPayload }
  | { type: 'upgrade-message'; payload: { message_id: string; personalized_text: string } }
  | { type: 'dismiss' }
  | { type: 'update-focus'; payload: FocusHighlightPayload }
  | { type: 'clear-focus' }
  | { type: 'update-goals'; payload: GoalProgressItem[] }
  | { type: 'set-mode'; payload: OverlayMode }
  | { type: 'set-focus-mode'; payload: { active: boolean; auto: boolean } }
  | { type: 'capture-state-changed'; payload: CaptureStatePayload }
  | { type: 'toggle-suggestions-panel'; payload?: boolean }
  | { type: 'capture-feedback'; payload: string }
  | { type: 'pointer-context-update'; payload: PointerContextPayload }
  | { type: 'set-suggestions'; payload: SuggestionViewDto[] }
  | { type: 'set-suggestion-surface'; payload: SuggestionSurfacePayload }
  | { type: 'remove-suggestion'; payload: string }
  | { type: 'detection-update'; payload: DetectionScenePayload }
  | { type: 'detection-clear' }
  | { type: 'detection-select'; payload: string | null }
  | { type: 'automation-confirm-request'; payload: PendingConfirmationDto }
  | { type: 'automation-confirm-dismiss' }
  | { type: 'codex-approval-request'; payload: CodexApprovalDto }
  | { type: 'codex-approval-dismiss' }
  | { type: 'set-fullscreen-policy'; payload: OverlayFullscreenPolicyPayload }

// #9637: exported so the reducer — the overlay's single state SSOT per the
// CLAUDE.md guardrail — can be unit-tested directly (13/23 branches had no
// automated coverage of any kind).
export const initialState: OverlayState = {
  mode: 'minimal',
  coaching: null,
  coachingQueue: [],
  focusHighlight: null,
  goals: [],
  captureState: { paused: true, indicator_visible: false, consent_granted: false, permitted: false },
  focusMode: false,
  focusModeAuto: false,
  suggestionsPanelOpen: false,
  suggestions: [],
  suggestionBadgeCount: 0,
  suggestionSurface: { placement: 'window-side-panel', anchor: null },
  captureFlashTimestamp: null,
  pointerContext: null,
  detectionScene: null,
  detectionSelectedId: null,
  pendingConfirmation: null,
  pendingCodexApproval: null,
  fullscreenPolicy: null,
}

export function reducer(state: OverlayState, action: OverlayAction): OverlayState {
  switch (action.type) {
    case 'show-coaching':
      if (state.coaching === null) {
        return { ...state, coaching: action.payload }
      }
      return { ...state, coachingQueue: [...state.coachingQueue, action.payload] }
    case 'upgrade-message':
      if (state.coaching?.message_id === action.payload.message_id) {
        return {
          ...state,
          coaching: { ...state.coaching, text: action.payload.personalized_text },
        }
      }
      return state
    case 'dismiss': {
      if (state.coachingQueue.length > 0) {
        const [next, ...rest] = state.coachingQueue
        return { ...state, coaching: next, coachingQueue: rest }
      }
      return { ...state, coaching: null }
    }
    case 'update-focus':
      if (state.detectionScene) return state
      return { ...state, focusHighlight: action.payload }
    case 'clear-focus':
      return { ...state, focusHighlight: null }
    case 'update-goals':
      return { ...state, goals: action.payload }
    case 'set-mode':
      return { ...state, mode: action.payload }
    case 'set-focus-mode':
      return { ...state, focusMode: action.payload.active, focusModeAuto: action.payload.auto }
    case 'capture-state-changed':
      return { ...state, captureState: action.payload }
    case 'toggle-suggestions-panel': {
      const isOpen = action.payload ?? !state.suggestionsPanelOpen
      return {
        ...state,
        suggestionsPanelOpen: isOpen,
        suggestionBadgeCount: isOpen ? 0 : state.suggestionBadgeCount,
      }
    }
    case 'set-suggestions': {
      const newCount = action.payload.length
      const oldCount = state.suggestions.length
      const delta = Math.max(0, newCount - oldCount)
      return {
        ...state,
        suggestions: action.payload,
        suggestionBadgeCount: state.suggestionsPanelOpen ? 0 : state.suggestionBadgeCount + delta,
      }
    }
    case 'set-suggestion-surface':
      return {
        ...state,
        suggestionSurface: action.payload,
      }
    case 'remove-suggestion':
      return {
        ...state,
        suggestions: state.suggestions.filter((s) => s.id !== action.payload),
      }
    case 'capture-feedback':
      return { ...state, captureFlashTimestamp: action.payload }
    case 'pointer-context-update':
      return { ...state, pointerContext: action.payload.enabled ? action.payload : null }
    case 'detection-update':
      return {
        ...state,
        detectionScene: action.payload,
        detectionSelectedId: null,
        focusHighlight: null,
      }
    case 'detection-clear':
      return { ...state, detectionScene: null, detectionSelectedId: null }
    case 'detection-select':
      return { ...state, detectionSelectedId: action.payload }
    case 'automation-confirm-request':
      return { ...state, pendingConfirmation: action.payload }
    case 'automation-confirm-dismiss':
      return { ...state, pendingConfirmation: null }
    case 'codex-approval-request':
      return { ...state, pendingCodexApproval: action.payload }
    case 'codex-approval-dismiss':
      return { ...state, pendingCodexApproval: null }
    case 'set-fullscreen-policy':
      return { ...state, fullscreenPolicy: action.payload }
    default:
      return state
  }
}

export function useOverlayEvents() {
  const [state, dispatch] = useReducer(reducer, initialState)

  useEffect(() => {
    const unlisten: Array<() => void> = []

    async function setup() {
      // Register each listener and track its unlisten fn immediately by pushing
      // onto `unlisten`. Previously the unlisten fns were collected into one
      // array only AFTER all registrations, so if any `await listen()` threw
      // mid-setup the remaining listeners were silently skipped AND the ones
      // already registered leaked (cleanup saw an empty array). Incremental push
      // + the surrounding try/catch make a partial failure both visible and
      // cleanly torn down by the effect's cleanup.
      try {
        const { listen } = await import('@tauri-apps/api/event')

        unlisten.push(
          await listen<CoachingPayload>('overlay:show-coaching', (e) => {
            dispatch({ type: 'show-coaching', payload: e.payload })
          }),
        )
        unlisten.push(
          await listen<{ message_id: string; personalized_text: string }>('overlay:upgrade-message', (e) => {
            dispatch({ type: 'upgrade-message', payload: e.payload })
          }),
        )
        unlisten.push(
          await listen('overlay:dismiss', () => {
            dispatch({ type: 'dismiss' })
          }),
        )
        unlisten.push(
          await listen<FocusHighlightPayload>('overlay:update-focus', (e) => {
            dispatch({ type: 'update-focus', payload: e.payload })
          }),
        )
        unlisten.push(
          await listen<{ goals: GoalProgressItem[] }>('overlay:update-goals', (e) => {
            dispatch({ type: 'update-goals', payload: e.payload.goals })
          }),
        )
        unlisten.push(
          await listen<{ mode: OverlayMode }>('overlay:set-mode', (e) => {
            dispatch({ type: 'set-mode', payload: e.payload.mode })
          }),
        )
        unlisten.push(
          await listen('overlay:clear-focus', () => {
            dispatch({ type: 'clear-focus' })
          }),
        )
        unlisten.push(
          await listen<CaptureStatePayload>('overlay:capture-state-changed', (e) => {
            dispatch({ type: 'capture-state-changed', payload: e.payload })
          }),
        )
        unlisten.push(
          await listen<FocusModePayload>('overlay:focus-mode', (e) => {
            dispatch({ type: 'set-focus-mode', payload: { active: e.payload.active, auto: e.payload.auto ?? false } })
          }),
        )
        // Explicit suggestions panel open/close state (#8847). Emitted with the
        // AUTHORITATIVE native state from the Cmd+Shift+S shortcut, the tracking
        // panel, and every toggle_suggestions_panel IPC call. Idempotent: the
        // reducer converges to `open`, so event timing cannot invert or lose the
        // native state (the former relative `overlay:toggle-suggestions` event —
        // lost when the WebView was destroyed by the idle policy — is retired).
        unlisten.push(
          await listen<{ open: boolean }>('overlay:set-suggestions-panel', (e) => {
            dispatch({ type: 'toggle-suggestions-panel', payload: !!e.payload.open })
          }),
        )
        // Suggestions changed — re-fetch
        unlisten.push(
          await listen<{ count: number }>('overlay:suggestions-changed', async () => {
            const { invoke } = await import('@tauri-apps/api/core')
            try {
              const suggestions = await invoke<SuggestionViewDto[]>('get_pending_suggestions')
              dispatch({ type: 'set-suggestions', payload: redactSuggestionViews(suggestions) })
            } catch (_e) {
              console.warn('get_pending_suggestions failed')
            }
          }),
        )
        // Capture feedback flash
        unlisten.push(
          await listen<{ timestamp: string }>('overlay:capture-feedback', (e) => {
            dispatch({ type: 'capture-feedback', payload: e.payload.timestamp })
          }),
        )
        unlisten.push(
          await listen<DetectionScenePayload>('overlay:detection-update', (e) => {
            dispatch({ type: 'detection-update', payload: e.payload })
          }),
        )
        unlisten.push(
          await listen('overlay:detection-clear', () => {
            dispatch({ type: 'detection-clear' })
          }),
        )
        unlisten.push(
          await listen<PendingConfirmationDto>('automation:confirm-request', (e) => {
            dispatch({ type: 'automation-confirm-request', payload: e.payload })
          }),
        )
        unlisten.push(
          await listen<SuggestionSurfacePayload>('overlay:set-suggestion-surface', (e) => {
            dispatch({ type: 'set-suggestion-surface', payload: e.payload })
          }),
        )
        // Codex app-server approval request (E21 #5044) → CodexApprovalModal
        unlisten.push(
          await listen<CodexApprovalDto>('codex:approval-request', (e) => {
            dispatch({ type: 'codex-approval-request', payload: e.payload })
          }),
        )
        unlisten.push(
          await listen<PointerContextPayload>('overlay:pointer-context-update', (e) => {
            dispatch({ type: 'pointer-context-update', payload: e.payload })
          }),
        )
        // Fullscreen overlay-policy decisions pushed from Rust: mirror the policy
        // in state so the overlay UI/diagnostics can reflect it (Rust still
        // enforces the policy itself by hiding the overlay window).
        unlisten.push(
          await listen<OverlayFullscreenPolicyPayload>('overlay:fullscreen-policy', (e) => {
            dispatch({ type: 'set-fullscreen-policy', payload: e.payload })
          }),
        )

        // Query actual backend state (overlay window may be created after state changes)
        try {
          const { invoke } = await import('@tauri-apps/api/core')
          const status = await invoke<CaptureStatePayload>('get_capture_status')
          dispatch({ type: 'capture-state-changed', payload: status })
        } catch (e) {
          console.warn('get_capture_status failed:', e)
        }
      } catch (e) {
        console.warn('overlay event listener setup failed:', e)
      }
    }

    setup()
    return () => {
      for (const fn of unlisten) fn()
    }
  }, [])

  return { state, dispatch }
}
