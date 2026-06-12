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
  OverlayMode,
  OverlayState,
  PendingConfirmationDto,
  PointerContextPayload,
  SuggestionSurfacePayload,
  SuggestionViewDto,
} from '../types'

type OverlayAction =
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

const initialState: OverlayState = {
  mode: 'minimal',
  coaching: null,
  coachingQueue: [],
  focusHighlight: null,
  goals: [],
  captureState: { paused: false, indicator_visible: false },
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
}

function reducer(state: OverlayState, action: OverlayAction): OverlayState {
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
    default:
      return state
  }
}

export function useOverlayEvents() {
  const [state, dispatch] = useReducer(reducer, initialState)

  useEffect(() => {
    let unlisten: Array<() => void> = []

    async function setup() {
      const { listen } = await import('@tauri-apps/api/event')

      const u1 = await listen<CoachingPayload>('overlay:show-coaching', (e) => {
        dispatch({ type: 'show-coaching', payload: e.payload })
      })
      const u2 = await listen<{ message_id: string; personalized_text: string }>('overlay:upgrade-message', (e) => {
        dispatch({ type: 'upgrade-message', payload: e.payload })
      })
      const u3 = await listen('overlay:dismiss', () => {
        dispatch({ type: 'dismiss' })
      })
      const u4 = await listen<FocusHighlightPayload>('overlay:update-focus', (e) => {
        dispatch({ type: 'update-focus', payload: e.payload })
      })
      const u5 = await listen<{ goals: GoalProgressItem[] }>('overlay:update-goals', (e) => {
        dispatch({ type: 'update-goals', payload: e.payload.goals })
      })
      const u6 = await listen<{ mode: OverlayMode }>('overlay:set-mode', (e) => {
        dispatch({ type: 'set-mode', payload: e.payload.mode })
      })

      const u7 = await listen('overlay:clear-focus', () => {
        dispatch({ type: 'clear-focus' })
      })

      const u8 = await listen<CaptureStatePayload>('overlay:capture-state-changed', (e) => {
        dispatch({ type: 'capture-state-changed', payload: e.payload })
      })

      const u9 = await listen<FocusModePayload>('overlay:focus-mode', (e) => {
        dispatch({ type: 'set-focus-mode', payload: { active: e.payload.active, auto: e.payload.auto ?? false } })
      })

      // u10: Suggestions panel toggle (from Cmd+Shift+S)
      const u10 = await listen('overlay:toggle-suggestions', () => {
        dispatch({ type: 'toggle-suggestions-panel' })
      })

      // u11: Explicit suggestions panel open/close request (from tracking panel)
      const u11 = await listen<{ open: boolean }>('overlay:set-suggestions-panel', (e) => {
        dispatch({ type: 'toggle-suggestions-panel', payload: !!e.payload.open })
      })

      // u12: Suggestions changed — re-fetch
      const u12 = await listen<{ count: number }>('overlay:suggestions-changed', async () => {
        const { invoke } = await import('@tauri-apps/api/core')
        try {
          const suggestions = await invoke<SuggestionViewDto[]>('get_pending_suggestions')
          dispatch({ type: 'set-suggestions', payload: redactSuggestionViews(suggestions) })
        } catch (_e) {
          console.warn('get_pending_suggestions failed')
        }
      })

      // u13: Capture feedback flash
      const u13 = await listen<{ timestamp: string }>('overlay:capture-feedback', (e) => {
        dispatch({ type: 'capture-feedback', payload: e.payload.timestamp })
      })

      const u14 = await listen<DetectionScenePayload>('overlay:detection-update', (e) => {
        dispatch({ type: 'detection-update', payload: e.payload })
      })

      const u15 = await listen('overlay:detection-clear', () => {
        dispatch({ type: 'detection-clear' })
      })

      const u16 = await listen<PendingConfirmationDto>('automation:confirm-request', (e) => {
        dispatch({ type: 'automation-confirm-request', payload: e.payload })
      })

      const u17 = await listen<SuggestionSurfacePayload>('overlay:set-suggestion-surface', (e) => {
        dispatch({ type: 'set-suggestion-surface', payload: e.payload })
      })

      // u18: Codex app-server approval request (E21 #5044) → CodexApprovalModal
      const u18 = await listen<CodexApprovalDto>('codex:approval-request', (e) => {
        dispatch({ type: 'codex-approval-request', payload: e.payload })
      })

      const u19 = await listen<PointerContextPayload>('overlay:pointer-context-update', (e) => {
        dispatch({ type: 'pointer-context-update', payload: e.payload })
      })

      unlisten = [u1, u2, u3, u4, u5, u6, u7, u8, u9, u10, u11, u12, u13, u14, u15, u16, u17, u18, u19]

      // Query actual backend state (overlay window may be created after state changes)
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        const status = await invoke<CaptureStatePayload>('get_capture_status')
        dispatch({ type: 'capture-state-changed', payload: status })
      } catch (e) {
        console.warn('get_capture_status failed:', e)
      }
    }

    setup()
    return () => {
      for (const fn of unlisten) fn()
    }
  }, [])

  return { state, dispatch }
}
