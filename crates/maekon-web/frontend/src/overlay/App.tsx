import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { MOD_KEY } from '../utils/platform'
import { AutomationConfirmModal } from './components/AutomationConfirmModal'
import { CaptureFlash } from './components/CaptureFlash'
import CoachingPopup from './components/CoachingPopup'
import { CodexApprovalModal } from './components/CodexApprovalModal'
import DetectionHeader from './components/DetectionHeader'
import DetectionOverlay from './components/DetectionOverlay'
import FocusHighlight from './components/FocusHighlight'
import { FocusModeIndicator } from './components/FocusModeIndicator'
import GoalProgressBar from './components/GoalProgressBar'
import HeatmapGhost from './components/HeatmapGhost'
import { PointerContextHighlight } from './components/PointerContextHighlight'
import { SuggestionBadge } from './components/SuggestionBadge'
import { SuggestionsPanel } from './components/SuggestionsPanel'
import { showToast, ToastContainer } from './components/Toast'
import { TrackingBorder } from './components/TrackingBorder'
import { useOverlayEvents } from './hooks/useOverlayEvents'
import { redactSuggestionViews } from './suggestionPrivacy'
import { buildSuggestionReplayEvent, recordSuggestionReplayEvent } from './suggestionReplay'
import type { CaptureStatePayload, SuggestionViewDto } from './types'

export default function OverlayApp() {
  const windowLabel = useTauriWindowLabel()

  if (!windowLabel.resolved) {
    return <div className="relative h-screen w-screen overflow-hidden" />
  }

  if (isTrackingBorderWindowLabel(windowLabel.value)) {
    return <TrackingBorderSurface windowLabel={windowLabel.value} />
  }

  return <InteractiveOverlaySurface />
}

interface TauriWindowLabelState {
  resolved: boolean
  value?: string
}

const ACTIVE_TRACKING_BORDER_STATE: CaptureStatePayload = {
  paused: false,
  indicator_visible: true,
  consent_granted: true,
  permitted: true,
}

function isTrackingBorderWindowLabel(windowLabel?: string): windowLabel is string {
  switch (windowLabel) {
    case 'tracking-border-top':
    case 'tracking-border-bottom':
    case 'tracking-border-left':
    case 'tracking-border-right':
      return true
    default:
      return false
  }
}

function useTauriWindowLabel(): TauriWindowLabelState {
  const [windowLabel, setWindowLabel] = useState<TauriWindowLabelState>({ resolved: false })

  useEffect(() => {
    let mounted = true

    ;(async () => {
      try {
        const { getCurrentWindow } = await import('@tauri-apps/api/window')
        if (mounted) setWindowLabel({ resolved: true, value: getCurrentWindow().label })
      } catch {
        if (mounted) setWindowLabel({ resolved: true, value: undefined })
      }
    })()

    return () => {
      mounted = false
    }
  }, [])

  return windowLabel
}

function TrackingBorderSurface({ windowLabel }: { windowLabel: string }) {
  return (
    <div className="relative h-screen w-screen overflow-hidden">
      <TrackingBorder captureState={ACTIVE_TRACKING_BORDER_STATE} windowLabel={windowLabel} />
    </div>
  )
}

function InteractiveOverlaySurface() {
  const { state, dispatch } = useOverlayEvents()
  const { t } = useTranslation()
  const isRich = state.mode === 'rich' || state.mode === 'adaptive'
  const [suggestionsPanelHydrated, setSuggestionsPanelHydrated] = useState(false)

  // A tracking-panel open request can lazily create this WebView. Tauri events
  // are not buffered, so hydrate the authoritative native state before the
  // write-back effect below can overwrite it with the reducer's initial false.
  useEffect(() => {
    let cancelled = false

    ;(async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        const open = await invoke<boolean>('get_suggestions_panel_open')
        if (!cancelled && typeof open === 'boolean') {
          dispatch({ type: 'toggle-suggestions-panel', payload: open })
        }
      } catch (e) {
        console.warn('get_suggestions_panel_open failed:', e)
      } finally {
        if (!cancelled) setSuggestionsPanelHydrated(true)
      }
    })()

    return () => {
      cancelled = true
    }
  }, [dispatch])

  // Sync suggestions panel open/close → Rust window resize.
  // When open, the overlay shrinks to a compact right-edge strip so it
  // doesn't block mouse events on the rest of the desktop.
  useEffect(() => {
    if (!suggestionsPanelHydrated) return

    ;(async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('toggle_suggestions_panel', { open: state.suggestionsPanelOpen })
      } catch (e) {
        console.warn('toggle_suggestions_panel failed:', e)
      }
    })()
  }, [state.suggestionsPanelOpen, suggestionsPanelHydrated])

  // Sync automation confirmation modal → Rust overlay mode.
  // Uses dedicated IPC so the mode priority system can recalculate
  // the correct window layout when the modal is dismissed.
  // A full-screen interactive modal (automation confirm OR codex approval, E21
  // #5044) needs the overlay window to capture events. Reuse the generic
  // full-screen-interactive toggle (`set_automation_confirm_mode`) — it is mode
  // priority, not automation-specific — OR'd across BOTH modal states so neither
  // clobbers the other's capture mode.
  // NOTE: click-through / event capture is NOT verifiable in cargo/Vitest —
  // flag for human visual confirmation.
  const fullScreenModalActive = !!state.pendingConfirmation || !!state.pendingCodexApproval
  useEffect(() => {
    ;(async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('toggle_automation_confirm', { active: fullScreenModalActive })
      } catch (e) {
        console.warn('toggle_automation_confirm failed:', e)
      }
    })()
  }, [fullScreenModalActive])

  // Show keyboard shortcut hint toast on first overlay activation.
  const overlayVisible = !!state.coaching || state.suggestionsPanelOpen
  const hintShownRef = useRef(false)

  useEffect(() => {
    if (!overlayVisible || hintShownRef.current) return
    const HINT_KEY = 'maekon-overlay-hints-shown'
    try {
      if (localStorage.getItem(HINT_KEY)) {
        hintShownRef.current = true
        return
      }
    } catch {
      return
    }

    const mod = MOD_KEY === '\u2318' ? '\u2318\u21e7' : 'Ctrl+Shift+'
    const timer = setTimeout(() => {
      showToast(t('overlay.hint', { mod }), 'info', 8000)
      try {
        localStorage.setItem(HINT_KEY, '1')
      } catch {
        /* non-critical — hint re-shows next session */
      }
      hintShownRef.current = true
    }, 500)

    return () => clearTimeout(timer)
  }, [overlayVisible, t])

  function handleClosePanel() {
    dispatch({ type: 'toggle-suggestions-panel', payload: false })
  }

  const handleRefreshSuggestions = useCallback(async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    try {
      const suggestions = await invoke<SuggestionViewDto[]>('get_pending_suggestions')
      dispatch({ type: 'set-suggestions', payload: redactSuggestionViews(suggestions) })
    } catch (e) {
      console.warn('get_pending_suggestions failed')
      throw e
    }
  }, [dispatch])

  const handleDetectionSelect = useCallback(
    (id: string | null) => {
      dispatch({ type: 'detection-select', payload: id })
    },
    [dispatch],
  )

  const handleDetectionRefresh = useCallback(async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    try {
      await invoke('refresh_detection_overlay')
    } catch (e) {
      console.warn('refresh_detection_overlay failed:', e)
    }
  }, [])

  const handleDetectionClose = useCallback(async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    try {
      await invoke('toggle_detection_overlay', { active: false })
    } catch (e) {
      console.warn('toggle_detection_overlay failed:', e)
    }
  }, [])

  const handleBadgeClick = useCallback(() => {
    void recordSuggestionReplayEvent(
      buildSuggestionReplayEvent({
        phase: 'marker_opened',
        placement: state.suggestionSurface.placement,
        anchor: state.suggestionSurface.anchor,
        suggestion: state.suggestions[0] ?? null,
      }),
    )
    dispatch({ type: 'toggle-suggestions-panel', payload: true })
  }, [dispatch, state.suggestionSurface.anchor, state.suggestionSurface.placement, state.suggestions])

  return (
    <div className="relative h-screen w-screen overflow-hidden">
      <PointerContextHighlight pointerContext={state.pointerContext} />

      {/* Detection mode header */}
      {state.detectionScene && (
        <DetectionHeader
          elementCount={state.detectionScene.element_count}
          onRefresh={handleDetectionRefresh}
          onClose={handleDetectionClose}
        />
      )}

      {/* Detection overlay boxes */}
      {state.detectionScene && (
        <DetectionOverlay
          scene={state.detectionScene}
          selectedId={state.detectionSelectedId}
          onSelect={handleDetectionSelect}
        />
      )}

      {/* Focus mode pill indicator (top center) */}
      <FocusModeIndicator active={state.focusMode} auto={state.focusModeAuto} />

      {/* Focus area highlight (when no detection mode) */}
      {!state.detectionScene && state.focusHighlight && <FocusHighlight highlight={state.focusHighlight} />}

      {/* Coaching popup (shown when a message is active) */}
      {state.coaching && <CoachingPopup message={state.coaching} autoDismissSecs={state.coaching.auto_dismiss_secs} />}

      {/* Suggestion badge (shown when panel is closed and there are new items) */}
      {!state.suggestionsPanelOpen && (
        <SuggestionBadge
          count={state.suggestionBadgeCount}
          anchor={state.suggestionSurface.anchor}
          onClick={handleBadgeClick}
        />
      )}

      {/* Suggestions panel (right side, slide in/out) */}
      <SuggestionsPanel
        open={state.suggestionsPanelOpen}
        suggestions={state.suggestions}
        onClose={handleClosePanel}
        onRefresh={handleRefreshSuggestions}
        placement={state.suggestionSurface.placement}
        anchor={state.suggestionSurface.anchor}
      />

      {/* Rich mode: goal progress bar at bottom */}
      {isRich && state.goals.length > 0 && <GoalProgressBar goals={state.goals} />}

      {/* Rich mode: attention heatmap ghost */}
      {isRich && <HeatmapGhost />}

      {/* Automation confirmation modal */}
      {state.pendingConfirmation && (
        <AutomationConfirmModal
          confirmation={state.pendingConfirmation}
          onDismiss={() => dispatch({ type: 'automation-confirm-dismiss' })}
        />
      )}

      {/* Codex app-server approval modal (E21 #5044) */}
      {state.pendingCodexApproval && (
        <CodexApprovalModal
          approval={state.pendingCodexApproval}
          onDismiss={() => dispatch({ type: 'codex-approval-dismiss' })}
        />
      )}

      {/* Manual capture feedback flash */}
      <CaptureFlash timestamp={state.captureFlashTimestamp} />

      {/* Toast notifications */}
      <ToastContainer />
    </div>
  )
}
