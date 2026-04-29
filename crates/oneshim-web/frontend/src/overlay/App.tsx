import { useCallback, useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { MOD_KEY } from '../utils/platform'
import { AutomationConfirmModal } from './components/AutomationConfirmModal'
import { CaptureFlash } from './components/CaptureFlash'
import CoachingPopup from './components/CoachingPopup'
import DetectionHeader from './components/DetectionHeader'
import DetectionOverlay from './components/DetectionOverlay'
import FocusHighlight from './components/FocusHighlight'
import { FocusModeIndicator } from './components/FocusModeIndicator'
import GoalProgressBar from './components/GoalProgressBar'
import HeatmapGhost from './components/HeatmapGhost'
import { SuggestionBadge } from './components/SuggestionBadge'
import { SuggestionsPanel } from './components/SuggestionsPanel'
import { showToast, ToastContainer } from './components/Toast'
import { useOverlayEvents } from './hooks/useOverlayEvents'
import type { SuggestionViewDto } from './types'

export default function OverlayApp() {
  const { state, dispatch } = useOverlayEvents()
  const { t } = useTranslation()
  const isRich = state.mode === 'rich' || state.mode === 'adaptive'

  // Sync suggestions panel open/close → Rust window resize.
  // When open, the overlay shrinks to a compact right-edge strip so it
  // doesn't block mouse events on the rest of the desktop.
  useEffect(() => {
    ;(async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('toggle_suggestions_panel', { open: state.suggestionsPanelOpen })
      } catch (e) {
        console.warn('toggle_suggestions_panel failed:', e)
      }
    })()
  }, [state.suggestionsPanelOpen])

  // Sync automation confirmation modal → Rust overlay mode.
  // Uses dedicated IPC so the mode priority system can recalculate
  // the correct window layout when the modal is dismissed.
  useEffect(() => {
    ;(async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('toggle_automation_confirm', { active: !!state.pendingConfirmation })
      } catch (e) {
        console.warn('toggle_automation_confirm failed:', e)
      }
    })()
  }, [state.pendingConfirmation])

  // Show keyboard shortcut hint toast on first overlay activation.
  const overlayVisible = !!state.coaching || state.suggestionsPanelOpen
  const hintShownRef = useRef(false)

  useEffect(() => {
    if (!overlayVisible || hintShownRef.current) return
    const HINT_KEY = 'oneshim-overlay-hints-shown'
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
      dispatch({ type: 'set-suggestions', payload: suggestions })
    } catch (e) {
      console.warn('get_pending_suggestions failed:', e)
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
    dispatch({ type: 'toggle-suggestions-panel', payload: true })
  }, [dispatch])

  return (
    <div className="relative h-screen w-screen overflow-hidden">
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
      {!state.suggestionsPanelOpen && <SuggestionBadge count={state.suggestionBadgeCount} onClick={handleBadgeClick} />}

      {/* Suggestions panel (right side, slide in/out) */}
      <SuggestionsPanel
        open={state.suggestionsPanelOpen}
        suggestions={state.suggestions}
        onClose={handleClosePanel}
        onRefresh={handleRefreshSuggestions}
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

      {/* Manual capture feedback flash */}
      <CaptureFlash timestamp={state.captureFlashTimestamp} />

      {/* Toast notifications */}
      <ToastContainer />
    </div>
  )
}
