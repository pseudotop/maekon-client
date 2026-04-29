// Dynamic imports for @tauri-apps/* — graceful degradation outside Tauri (ADR-004)
import { Brain, Camera, Crosshair, LayoutDashboard, Lightbulb, Settings, WifiOff } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

interface CaptureState {
  paused: boolean
  indicator_visible: boolean
}

interface ConnectionStatus {
  server: boolean
  llm: boolean
  cli: boolean
}

interface SceneAnalysisResult {
  app_name: string
  window_title: string
  accessibility?: { focused_element?: { role: string; label?: string }; element_count: number }
  ocr_regions: Array<{ text: string }>
  gui_elements: Array<{
    role: string
    label?: string
    bounds?: [number, number, number, number]
    type_confidence: number
  }>
  work_type?: string
}

/** Lazy-loaded invoke — graceful degradation outside Tauri (ADR-004). */
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: inv } = await import('@tauri-apps/api/core')
  return inv<T>(cmd, args)
}

const COLLAPSED_WIDTH = 260
const COLLAPSED_HEIGHT = 36
const EXPANDED_WIDTH = 320
const EXPANDED_HEIGHT = 310

export function App() {
  const { t } = useTranslation()
  const [state, setState] = useState<CaptureState>({ paused: false, indicator_visible: true })
  const [conn, setConn] = useState<ConnectionStatus>({ server: false, llm: false, cli: false })
  const [expanded, setExpanded] = useState(false)
  const [feedback, setFeedback] = useState<string | null>(null)
  const [sceneResult, setSceneResult] = useState<SceneAnalysisResult | null>(null)
  const positionSaveTimer = useRef<number | null>(null)
  const feedbackTimer = useRef<number | null>(null)

  const showFeedback = useCallback((msg: string) => {
    setFeedback(msg)
    if (feedbackTimer.current) clearTimeout(feedbackTimer.current)
    feedbackTimer.current = window.setTimeout(() => setFeedback(null), 3500)
  }, [])

  // Explicit drag initiation — backup for data-tauri-drag-region
  const handleDragMouseDown = useCallback(async (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest('button')) return
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      getCurrentWindow().startDragging()
    } catch (err) {
      console.debug('startDragging failed:', err)
    }
  }, [])

  useEffect(() => {
    let disposed = false
    const unlistens: Array<() => void> = []

    ;(async () => {
      const { listen: listenAsync } = await import('@tauri-apps/api/event')
      if (disposed) return

      unlistens.push(
        await listenAsync<CaptureState>('overlay:capture-state-changed', (e) => {
          setState(e.payload)
        }),
      )
      if (disposed) {
        unlistens.forEach((fn) => {
          fn()
        })
        return
      }

      unlistens.push(
        await listenAsync<ConnectionStatus>('overlay:connection-changed', (e) => {
          setConn(e.payload)
        }),
      )
      if (disposed) {
        unlistens.forEach((fn) => {
          fn()
        })
        return
      }
    })()

    ;(async () => {
      try {
        const { invoke: inv } = await import('@tauri-apps/api/core')
        inv<CaptureState>('get_capture_status')
          .then(setState)
          .catch((e) => {
            console.warn('get_capture_status failed:', e)
            showFeedback(t('trackingPanel.statusUnavailable'))
          })
        inv<ConnectionStatus>('get_connection_status')
          .then(setConn)
          .catch((e) => {
            console.warn('get_connection_status failed:', e)
            showFeedback(t('trackingPanel.connectionUnavailable'))
          })
        // Restore saved position
        const pos = await inv<string | null>('get_panel_position').catch(() => null)
        if (pos) {
          const [x, y] = pos.split(',').map(Number)
          if (Number.isFinite(x) && Number.isFinite(y)) {
            const { getCurrentWindow } = await import('@tauri-apps/api/window')
            const { LogicalPosition } = await import('@tauri-apps/api/dpi')
            getCurrentWindow()
              .setPosition(new LogicalPosition(x, y))
              .catch((e) => console.debug('setPosition failed:', e))
          }
        }
      } catch {
        /* not in Tauri */
      }
    })()

    return () => {
      disposed = true
      unlistens.forEach((fn) => {
        fn()
      })
    }
  }, [showFeedback, t])

  // Save position on window move (debounced)
  useEffect(() => {
    let unlisten: (() => void) | undefined
    ;(async () => {
      try {
        const { listen: listenMove } = await import('@tauri-apps/api/event')
        unlisten = await listenMove('tauri://move', (e) => {
          if (positionSaveTimer.current) clearTimeout(positionSaveTimer.current)
          const payload = e.payload as { x?: number; y?: number } | undefined
          if (payload && typeof payload.x === 'number' && typeof payload.y === 'number') {
            positionSaveTimer.current = window.setTimeout(async () => {
              try {
                const { invoke: inv } = await import('@tauri-apps/api/core')
                await inv('save_panel_position', { x: payload.x, y: payload.y })
              } catch (err) {
                console.debug('save_panel_position failed:', err)
              }
            }, 1000)
          }
        })
      } catch {
        /* not in Tauri */
      }
    })()
    return () => unlisten?.()
  }, [])

  const toggleExpanded = useCallback(async () => {
    const next = !expanded
    setExpanded(next)
    const w = next ? EXPANDED_WIDTH : COLLAPSED_WIDTH
    const h = next ? EXPANDED_HEIGHT : COLLAPSED_HEIGHT
    const heightDiff = EXPANDED_HEIGHT - COLLAPSED_HEIGHT

    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      const { LogicalPosition, LogicalSize } = await import('@tauri-apps/api/dpi')
      const win = getCurrentWindow()
      const scale = await win.scaleFactor()

      if (next) {
        const pos = await win.outerPosition()
        await win.setPosition(new LogicalPosition(pos.x / scale, pos.y / scale - heightDiff))
        await win.setSize(new LogicalSize(w, h))
      } else {
        await win.setSize(new LogicalSize(w, h))
        const pos = await win.outerPosition()
        await win.setPosition(new LogicalPosition(pos.x / scale, pos.y / scale + heightDiff))
      }
    } catch (e) {
      console.warn('toggleExpanded failed:', e)
    }
  }, [expanded])

  const handleManualCapture = useCallback(async () => {
    try {
      await invoke('trigger_manual_capture')
      showFeedback(t('trackingPanel.captured'))
    } catch (e) {
      console.warn('trigger_manual_capture failed:', e)
      showFeedback(t('trackingPanel.captureFailed'))
    }
  }, [showFeedback, t])

  const handleSceneAnalysis = useCallback(async () => {
    try {
      showFeedback(t('trackingPanel.analyzing'))
      const result = await invoke<SceneAnalysisResult>('analyze_current_scene')
      setSceneResult(result)
      showFeedback(`${result.app_name} — ${result.accessibility?.element_count ?? 0} ${t('trackingPanel.elements')}`)
      // Auto-dismiss scene result after 10s
      setTimeout(() => setSceneResult(null), 10000)
    } catch (e) {
      console.warn('analyze_current_scene failed:', e)
      showFeedback(t('trackingPanel.analysisFailed'))
    }
  }, [showFeedback, t])

  const handleToggleFocus = useCallback(async () => {
    try {
      const status = await invoke<{ active: boolean }>('get_focus_mode_status')
      await invoke('toggle_focus_mode', { active: !status.active, durationMinutes: 25 })
      showFeedback(status.active ? t('trackingPanel.focusOff') : t('trackingPanel.focus25m'))
    } catch (e) {
      console.warn('toggle_focus_mode failed:', e)
      showFeedback(t('trackingPanel.focusToggleFailed'))
    }
  }, [showFeedback, t])

  const handleSuggestions = useCallback(async () => {
    try {
      const { emit } = await import('@tauri-apps/api/event')
      // Emit toggle event — the overlay's useEffect handles window resize
      // via toggle_suggestions_panel IPC (compact panel mode).
      await emit('overlay:toggle-suggestions')
      showFeedback(t('trackingPanel.suggestionsOpened'))
    } catch (e) {
      console.warn('overlay:toggle-suggestions failed:', e)
      showFeedback(t('trackingPanel.suggestionsUnavailable'))
    }
  }, [showFeedback, t])

  if (!state.indicator_visible) return null

  const connCount = [conn.server, conn.llm, conn.cli].filter(Boolean).length
  const allConnected = connCount === 3
  const isLocalMode = connCount === 0
  const expandedStatusMessage = feedback ?? (isLocalMode ? t('trackingPanel.offlineMessage') : null)

  return (
    <div
      data-tauri-drag-region
      className={`flex select-none flex-col overflow-hidden rounded-xl bg-black/80 text-white text-xs backdrop-blur-md ${state.paused ? '' : 'animate-panel-glow'}`}
      style={
        state.paused
          ? {
              boxShadow: 'inset 0 0 12px 3px rgb(var(--content-muted) / 0.25)',
              border: '1.5px solid rgb(var(--content-muted) / 0.3)',
            }
          : undefined
      }
    >
      {/* Collapsed bar */}
      <div
        role="toolbar"
        data-tauri-drag-region
        onMouseDown={handleDragMouseDown}
        className="flex cursor-move items-center gap-2 px-3 py-2"
      >
        <span
          className={`h-2 w-2 shrink-0 rounded-full ${state.paused ? 'bg-status-connecting' : 'bg-status-connected'}`}
        />
        {!allConnected && (
          <span className="h-2 w-2 shrink-0 rounded-full bg-status-error" title={`${connCount}/3 connected`} />
        )}
        <span data-tauri-drag-region className="flex-1 truncate">
          {state.paused ? t('trackingPanel.paused') : (feedback ?? t('trackingPanel.capturing'))}
        </span>

        <button
          type="button"
          onClick={() => invoke('toggle_capture_pause')}
          className="rounded px-1.5 py-0.5 transition-colors hover:bg-white/20"
          title={state.paused ? t('trackingPanel.resume') : t('trackingPanel.pause')}
        >
          {state.paused ? '\u25B6' : '\u23F8'}
        </button>
        <button
          type="button"
          onClick={toggleExpanded}
          className="rounded px-1.5 py-0.5 transition-colors hover:bg-white/20"
          title={expanded ? t('trackingPanel.collapse') : t('trackingPanel.expand')}
        >
          {expanded ? '\u2501' : '\u229E'}
        </button>
        <button
          type="button"
          onClick={() => invoke('set_indicator_visible', { visible: false })}
          className="rounded px-1 py-0.5 transition-colors hover:bg-white/20"
          title={t('trackingPanel.hide')}
        >
          {'\u2715'}
        </button>
      </div>

      {/* Expanded panel */}
      {expanded && (
        <div data-tauri-drag-region className="flex cursor-move flex-col gap-1 border-white/10 border-t px-3 pt-1 pb-3">
          <ActionButton
            icon={<LayoutDashboard size={14} />}
            label={t('trackingPanel.openDashboard')}
            onClick={() => invoke('show_main_window')}
          />
          <ActionButton
            icon={<Camera size={14} />}
            label={t('trackingPanel.manualCapture')}
            onClick={handleManualCapture}
          />
          <ActionButton
            icon={<Brain size={14} />}
            label={t('trackingPanel.sceneAnalysis')}
            onClick={handleSceneAnalysis}
          />
          <ActionButton
            icon={<Lightbulb size={14} />}
            label={t('trackingPanel.aiSuggestions')}
            onClick={handleSuggestions}
          />
          <ActionButton
            icon={<Crosshair size={14} />}
            label={t('trackingPanel.focusMode')}
            onClick={handleToggleFocus}
          />

          {/* Connection status + local service lane indicator */}
          <div data-tauri-drag-region className="mt-2 border-white/10 border-t pt-2">
            {expandedStatusMessage && (
              <output aria-live="polite" className="mb-1.5 flex items-center gap-1.5 text-[10px] text-semantic-warning">
                {isLocalMode && !feedback && <WifiOff size={10} />}
                <span>{expandedStatusMessage}</span>
              </output>
            )}
            <div data-tauri-drag-region className="flex items-center justify-between text-[10px] text-white/60">
              <div className="flex items-center gap-3">
                <StatusDot connected={conn.server} label={t('trackingPanel.server')} />
                <StatusDot connected={conn.llm} label="LLM" />
                <StatusDot connected={conn.cli} label="CLI" />
              </div>
              <button
                type="button"
                onClick={() => invoke('show_main_window')}
                className="rounded p-0.5 transition-colors hover:bg-white/10"
                title={t('trackingPanel.openSettings')}
              >
                <Settings size={10} />
              </button>
            </div>
          </div>

          {/* Scene analysis result (auto-dismisses after 10s) */}
          {sceneResult && (
            <div className="mt-1 rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-[10px]">
              <div className="flex items-center justify-between">
                <span className="truncate font-medium text-white/90">
                  {sceneResult.app_name} — {sceneResult.window_title}
                </span>
                <button
                  type="button"
                  onClick={() => setSceneResult(null)}
                  className="ml-1 text-white/40 hover:text-white/80"
                >
                  &times;
                </button>
              </div>
              <div className="mt-1 flex gap-3 text-white/50">
                <span>
                  {sceneResult.accessibility?.element_count ?? 0} {t('trackingPanel.elements')}
                </span>
                <span>{sceneResult.ocr_regions.length} OCR</span>
                {sceneResult.work_type && <span>{sceneResult.work_type}</span>}
              </div>
              {sceneResult.accessibility?.focused_element && (
                <div className="mt-0.5 truncate text-white/40">
                  {t('trackingPanel.focus')}: {sceneResult.accessibility.focused_element.role}
                  {sceneResult.accessibility.focused_element.label &&
                    ` "${sceneResult.accessibility.focused_element.label}"`}
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

function ActionButton({
  icon,
  label,
  onClick,
  disabled,
}: {
  icon: React.ReactNode
  label: string
  onClick?: () => void
  disabled?: boolean
}) {
  const { t } = useTranslation()
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={`flex items-center gap-2 rounded-lg px-2 py-1.5 text-left text-white/80 transition-colors ${
        disabled ? 'cursor-not-allowed opacity-40' : 'hover:bg-white/10 active:bg-white/20'
      }`}
      title={disabled ? t('trackingPanel.comingSoon') : label}
    >
      <span className="flex w-5 items-center justify-center">{icon}</span>
      <span>{label}</span>
    </button>
  )
}

function StatusDot({ connected, label }: { connected: boolean; label: string }) {
  return (
    <span className="flex items-center gap-1">
      <span className={`h-1.5 w-1.5 rounded-full ${connected ? 'bg-status-connected' : 'bg-status-error'}`} />
      {label}
    </span>
  )
}
