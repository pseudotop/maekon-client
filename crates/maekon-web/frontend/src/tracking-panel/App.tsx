// Dynamic imports for @tauri-apps/* — graceful degradation outside Tauri (ADR-004)
import {
  Activity,
  Brain,
  Camera,
  ChevronRight,
  Crosshair,
  LayoutDashboard,
  Lightbulb,
  Pin,
  Plus,
  Power,
  Settings,
  WifiOff,
} from 'lucide-react'
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
const EXPANDED_HEIGHT = 430

export function App() {
  const { t } = useTranslation()
  const [state, setState] = useState<CaptureState>({ paused: false, indicator_visible: true })
  const [conn, setConn] = useState<ConnectionStatus>({ server: false, llm: false, cli: false })
  const [expanded, setExpanded] = useState(false)
  const [feedback, setFeedback] = useState<string | null>(null)
  const [sceneResult, setSceneResult] = useState<SceneAnalysisResult | null>(null)
  const [captureCount, setCaptureCount] = useState(0)
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
      setCaptureCount((count) => count + 1)
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
      await emit('overlay:set-suggestions-panel', { open: true })
      try {
        await invoke('toggle_suggestions_panel', { open: true })
      } catch (e) {
        console.debug('toggle_suggestions_panel fallback failed:', e)
      }
      showFeedback(t('trackingPanel.suggestionsOpened'))
    } catch (e) {
      console.warn('open suggestions panel failed:', e)
      showFeedback(t('trackingPanel.suggestionsUnavailable'))
    }
  }, [showFeedback, t])

  const handleOpenMaekon = useCallback(async () => {
    await invoke('show_main_window')
  }, [])

  const handleOpenChat = useCallback(async () => {
    try {
      const { emit } = await import('@tauri-apps/api/event')
      await invoke('show_main_window')
      await emit('navigate:chat', {})
      showFeedback(t('trackingPanel.chatOpened'))
    } catch (e) {
      console.warn('open chat failed:', e)
      showFeedback(t('trackingPanel.connectionUnavailable'))
    }
  }, [showFeedback, t])

  const handleQuit = useCallback(async () => {
    try {
      await invoke('simulate_tray_action', { action: 'quit' })
      showFeedback(t('trackingPanel.quitRequested'))
    } catch (e) {
      console.warn('quit request failed:', e)
      showFeedback(t('trackingPanel.quitUnavailable'))
    }
  }, [showFeedback, t])

  if (!state.indicator_visible) return null

  const connCount = [conn.server, conn.llm, conn.cli].filter(Boolean).length
  const allConnected = connCount === 3
  const isLocalMode = connCount === 0
  const expandedStatusMessage = feedback ?? (isLocalMode ? t('trackingPanel.offlineMessage') : null)
  const runningLabel = state.paused ? t('trackingPanel.screenContextPaused') : t('trackingPanel.screenContextReady')
  const pendingSuggestionCount = sceneResult ? 1 : 0

  return (
    <div
      data-tauri-drag-region
      data-visual-region="floating-bar-anchor"
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
        <span data-tauri-drag-region data-visual-region="screen-context-status" className="flex-1 truncate">
          {state.paused ? t('trackingPanel.paused') : (feedback ?? t('trackingPanel.capturing'))}
        </span>

        <button
          type="button"
          onClick={handleSuggestions}
          aria-label={`${pendingSuggestionCount} ${t('trackingPanel.aiSuggestions')}`}
          data-visual-region="collapsed-suggestion-count"
          className="inline-flex min-w-7 items-center justify-center gap-1 rounded-full bg-white/10 px-1.5 py-0.5 text-[10px] transition-colors hover:bg-white/20"
          title={t('trackingPanel.aiSuggestions')}
        >
          <Lightbulb size={10} />
          <span>{pendingSuggestionCount}</span>
        </button>
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
        <section
          data-tauri-drag-region
          aria-label={t('trackingPanel.floatingMenu')}
          data-visual-region="keyboard-a11y-action"
          className="flex cursor-move flex-col gap-2 border-white/10 border-t px-3 pt-2 pb-3"
        >
          <MenuSection title={t('trackingPanel.running')}>
            <CommandMenuItem
              icon={<Activity size={14} />}
              label={runningLabel}
              meta={isLocalMode ? t('trackingPanel.localMode') : t('trackingPanel.captureServicesReady')}
            />
            {expandedStatusMessage && (
              <output aria-live="polite" className="flex items-center gap-1.5 px-2 text-[10px] text-semantic-warning">
                {isLocalMode && !feedback && <WifiOff size={10} />}
                <span>{expandedStatusMessage}</span>
              </output>
            )}
          </MenuSection>

          <MenuSection title={t('trackingPanel.pinned')}>
            <CommandMenuItem
              icon={<Pin size={14} />}
              label={t('trackingPanel.extractTasks')}
              meta={t('trackingPanel.extractTasksMeta')}
              onClick={handleSuggestions}
            />
            <CommandMenuItem
              icon={<Camera size={14} />}
              label={t('trackingPanel.manualCapture')}
              meta={t('trackingPanel.captureCurrentContextMeta')}
              onClick={handleManualCapture}
            />
          </MenuSection>

          <MenuSection title={t('trackingPanel.recent')}>
            <CommandMenuItem
              icon={<Brain size={14} />}
              label={t('trackingPanel.reviewCurrentWindow')}
              meta={t('trackingPanel.reviewCurrentWindowMeta')}
              onClick={handleSceneAnalysis}
            />
            <CommandMenuItem
              icon={<Lightbulb size={14} />}
              label={t('trackingPanel.aiSuggestions')}
              meta={t('trackingPanel.aiSuggestionsMeta')}
              onClick={handleSuggestions}
            />
            <CommandMenuItem
              icon={<Crosshair size={14} />}
              label={t('trackingPanel.focusMode')}
              meta={t('trackingPanel.focusModeMeta')}
              onClick={handleToggleFocus}
            />
          </MenuSection>

          <MenuSection title={t('trackingPanel.usage')}>
            <UsageRow label={t('trackingPanel.captureUsage', { count: captureCount })} />
            <UsageRow label={t('trackingPanel.serviceLanesConnected', { count: connCount, total: 3 })} />
            <div
              data-tauri-drag-region
              data-visual-region="provider-health-dots"
              className="flex items-center justify-between px-2 text-[10px] text-white/60"
            >
              <div className="flex items-center gap-3">
                <StatusDot connected={conn.server} label={t('trackingPanel.server')} />
                <StatusDot connected={conn.llm} label="LLM" />
                <StatusDot connected={conn.cli} label="CLI" />
              </div>
              <button
                type="button"
                onClick={handleOpenMaekon}
                className="rounded p-0.5 transition-colors hover:bg-white/10"
                title={t('trackingPanel.openSettings')}
              >
                <Settings size={10} />
              </button>
            </div>
          </MenuSection>

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

          <div
            data-tauri-drag-region
            data-visual-region="quick-actions"
            className="mt-0.5 border-white/10 border-t pt-1"
          >
            <CommandMenuItem icon={<Plus size={14} />} label={t('trackingPanel.newChat')} onClick={handleOpenChat} />
            <CommandMenuItem
              icon={<LayoutDashboard size={14} />}
              label={t('trackingPanel.openMaekon')}
              onClick={handleOpenMaekon}
            />
            <CommandMenuItem
              icon={<Power size={14} />}
              label={t('trackingPanel.quitMaekon')}
              onClick={handleQuit}
              danger
            />
          </div>
        </section>
      )}
    </div>
  )
}

function MenuSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section data-tauri-drag-region className="border-white/10 border-b pb-1 last:border-b-0">
      <h2 data-tauri-drag-region className="px-2 pb-0.5 font-medium text-[10px] text-white/45">
        {title}
      </h2>
      <div data-tauri-drag-region className="flex flex-col gap-0.5">
        {children}
      </div>
    </section>
  )
}

function CommandMenuItem({
  icon,
  label,
  meta,
  onClick,
  disabled,
  danger,
  visualRegion,
}: {
  icon: React.ReactNode
  label: string
  meta?: string
  onClick?: () => void
  disabled?: boolean
  danger?: boolean
  visualRegion?: string
}) {
  const { t } = useTranslation()
  if (!onClick) {
    return (
      <div
        data-tauri-drag-region
        data-visual-region={visualRegion}
        className="flex items-start gap-2 rounded-md px-2 py-1 text-left text-white/80"
      >
        <span className="mt-0.5 flex w-5 items-center justify-center text-white/70">{icon}</span>
        <span className="min-w-0 flex-1">
          <span className="block truncate">{label}</span>
          {meta && <span className="block truncate text-[10px] text-white/45">{meta}</span>}
        </span>
      </div>
    )
  }

  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      data-visual-region={visualRegion}
      className={`flex items-start gap-2 rounded-md px-2 py-1 text-left transition-colors ${
        disabled ? 'cursor-not-allowed opacity-40' : 'hover:bg-white/10 active:bg-white/20'
      } ${danger ? 'text-semantic-error' : 'text-white/80'}`}
      title={disabled ? t('trackingPanel.comingSoon') : label}
    >
      <span className="mt-0.5 flex w-5 items-center justify-center">{icon}</span>
      <span className="min-w-0 flex-1">
        <span className="block truncate">{label}</span>
        {meta && <span className="block truncate text-[10px] text-white/45">{meta}</span>}
      </span>
      <ChevronRight size={12} className="mt-1 text-white/35" />
    </button>
  )
}

function UsageRow({ label }: { label: string }) {
  return (
    <div data-tauri-drag-region className="px-2 py-0.5 text-[10px] text-white/60">
      {label}
    </div>
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
