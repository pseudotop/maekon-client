import { Loader2, Lock, RefreshCw } from 'lucide-react'
import type { CSSProperties } from 'react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { translateError, type WireErrorLocale } from '../../i18n/translateError'
import { iconSize, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { buildSuggestionReplayEvent, recordSuggestionReplayEvent } from '../suggestionReplay'
import type { SuggestionGuiAnchorPayload, SuggestionSurfacePlacement, SuggestionViewDto } from '../types'
import { SuggestionHistory } from './SuggestionHistory'
import { SuggestionItem } from './SuggestionItem'
import { SuggestionReplayTrail } from './SuggestionReplayTrail'
import { SuggestionStats } from './SuggestionStats'
import { showToast } from './Toast'

// ADR-019 Follow-up #3: Route through translateError to preserve and localize the IpcError wire code.
// The previous errorMessage helper discarded the code and exposed only the raw English message.
function errorMessage(e: unknown, locale: WireErrorLocale): string {
  return translateError(e, locale)
}

interface SuggestionsPanelProps {
  open: boolean
  suggestions: SuggestionViewDto[]
  onClose: () => void
  onRefresh: () => Promise<void> | void
  placement?: SuggestionSurfacePlacement
  anchor?: SuggestionGuiAnchorPayload | null
}

const SURFACE_MARGIN = 12
const PANEL_WIDTH = 320
const PANEL_MIN_HEIGHT = 260

function viewportWidth() {
  return typeof window === 'undefined' ? 1024 : window.innerWidth
}

function viewportHeight() {
  return typeof window === 'undefined' ? 768 : window.innerHeight
}

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), Math.max(min, max))
}

function sideAwareLeft(preferredRight: number, sourceLeft: number) {
  const maxLeft = viewportWidth() - PANEL_WIDTH - SURFACE_MARGIN
  if (preferredRight <= maxLeft) {
    return clamp(preferredRight, SURFACE_MARGIN, maxLeft)
  }
  return clamp(sourceLeft - PANEL_WIDTH - SURFACE_MARGIN, SURFACE_MARGIN, maxLeft)
}

function targetBreadcrumb(anchor?: SuggestionGuiAnchorPayload | null) {
  if (!anchor) return null
  return `${anchor.active_app} > ${anchor.target_entity.label} ${anchor.target_entity.value}`.trim()
}

function suggestionMatchesAnchor(item: SuggestionViewDto, anchor?: SuggestionGuiAnchorPayload | null) {
  const scope = item.context_scope
  if (!anchor || !scope) return true

  if (scope.target_id && scope.target_id !== anchor.target_entity.entity_id) return false
  if (scope.app_name && scope.app_name !== anchor.active_app) return false
  if (scope.window_title && scope.window_title !== anchor.active_process.window_title) return false

  return true
}

function hasSuggestionScope(item: SuggestionViewDto) {
  const scope = item.context_scope
  return Boolean(scope?.target_id || scope?.app_name || scope?.window_title)
}

function placementStyle(
  placement: SuggestionSurfacePlacement,
  anchor?: SuggestionGuiAnchorPayload | null,
): CSSProperties | undefined {
  if (!anchor) return undefined
  if (placement === 'adjacent-popover') {
    const bounds = anchor.highlight.bounds
    const top = clamp(bounds.y - 8, SURFACE_MARGIN, viewportHeight() - PANEL_MIN_HEIGHT - SURFACE_MARGIN)
    return {
      left: sideAwareLeft(bounds.x + bounds.width + SURFACE_MARGIN, bounds.x),
      top,
    }
  }
  if (placement === 'window-side-panel') {
    const bounds = anchor.active_process.window_bounds
    const maxHeight = Math.min(Math.max(PANEL_MIN_HEIGHT, bounds.height), viewportHeight() - SURFACE_MARGIN * 2)
    return {
      left: sideAwareLeft(bounds.x + bounds.width + SURFACE_MARGIN, bounds.x),
      top: clamp(bounds.y, SURFACE_MARGIN, viewportHeight() - maxHeight - SURFACE_MARGIN),
      maxHeight,
    }
  }
  return undefined
}

/**
 * Focus management: Overlay panels run in compact Tauri WebView windows.
 * - Escape key dismisses the panel (global keydown listener).
 * - No focus trap needed — the panel IS the entire window content.
 * - Interactive elements use standard tab order within the panel.
 */
export function SuggestionsPanel({
  open,
  suggestions,
  onClose,
  onRefresh,
  placement = 'window-side-panel',
  anchor,
}: SuggestionsPanelProps) {
  const { t, i18n } = useTranslation()
  // ADR-019 Follow-up #3: Map the current UI locale to the wire-error locale (same pattern as BugReportWizard).
  const errorLocale: WireErrorLocale = i18n.language?.startsWith('ko') ? 'ko' : 'en'
  const [error, setError] = useState<string | null>(null)
  const [refreshing, setRefreshing] = useState(false)
  const refreshingRef = useRef(false)
  const recordedProposalKeyRef = useRef<string | null>(null)
  const [activeTab, setActiveTab] = useState<'active' | 'history' | 'stats'>('active')

  // Source filter with localStorage persistence
  const [sourceFilter, setSourceFilter] = useState<Set<string>>(() => {
    try {
      const saved = localStorage.getItem('suggestion-source-filter')
      if (saved) {
        const parsed = JSON.parse(saved) as string[]
        // Migrate: old filters without 'rule' get it appended
        if (!parsed.includes('rule')) {
          parsed.push('rule')
          localStorage.setItem('suggestion-source-filter', JSON.stringify(parsed))
        }
        return new Set(parsed)
      }
      return new Set(['server', 'local', 'rule'])
    } catch {
      return new Set(['server', 'local', 'rule'])
    }
  })

  const filteredSuggestions = useMemo(() => {
    const sourceFiltered = suggestions.filter((s) => sourceFilter.has(s.source))
    if (!anchor) return sourceFiltered

    const scopedAnchorMatches = sourceFiltered.filter(
      (s) => hasSuggestionScope(s) && suggestionMatchesAnchor(s, anchor),
    )
    if (scopedAnchorMatches.length > 0) return scopedAnchorMatches

    return sourceFiltered.filter((s) => suggestionMatchesAnchor(s, anchor))
  }, [suggestions, sourceFilter, anchor])

  const primarySuggestion = filteredSuggestions[0]

  const toggleSource = (source: string) => {
    setSourceFilter((prev) => {
      const next = new Set(prev)
      if (next.has(source)) next.delete(source)
      else next.add(source)
      localStorage.setItem('suggestion-source-filter', JSON.stringify([...next]))
      return next
    })
  }

  const refreshSuggestions = useCallback(async () => {
    if (refreshingRef.current) return
    refreshingRef.current = true
    setRefreshing(true)
    try {
      await Promise.resolve(onRefresh())
      setError(null)
    } catch (_e) {
      console.warn('SuggestionsPanel refresh failed')
      setError(t('suggestions.loadError', 'Could not load suggestions.'))
    } finally {
      refreshingRef.current = false
      setRefreshing(false)
    }
  }, [onRefresh, t])

  useEffect(() => {
    if (!open) return
    void refreshSuggestions()
  }, [open, refreshSuggestions])

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape' && open) onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  useEffect(() => {
    if (!open || !anchor || !primarySuggestion) return
    const key = `${placement}:${anchor.target_entity.entity_id}:${primarySuggestion.id}`
    if (recordedProposalKeyRef.current === key) return
    recordedProposalKeyRef.current = key
    void recordSuggestionReplayEvent(
      buildSuggestionReplayEvent({
        phase: 'proposal_visible',
        placement,
        anchor,
        suggestion: primarySuggestion,
      }),
    )
  }, [anchor, open, placement, primarySuggestion])

  async function handleAction(id: string, action: 'accept' | 'reject' | 'defer' | 'explain', snoozeMinutes?: number) {
    const actedSuggestion = filteredSuggestions.find((item) => item.id === id)
    if (action === 'explain') {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('explain_suggestion_in_chat', { suggestionId: id })
        showToast(t('suggestions.openingInChat', 'Opening in chat...'), 'info')
      } catch (e) {
        showToast(errorMessage(e, errorLocale), 'error')
      }
      return
    }
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      // Tauri v2 auto-converts camelCase JS -> snake_case Rust params
      await invoke('submit_suggestion_feedback', { suggestionId: id, action, snoozeMinutes })
      await recordSuggestionReplayEvent(
        buildSuggestionReplayEvent({
          phase: 'feedback_submitted',
          placement,
          anchor,
          suggestion: actedSuggestion ?? null,
          action,
        }),
      )
      setError(null)
      await Promise.resolve(onRefresh())
      showToast(
        action === 'accept'
          ? t('suggestions.toastAccepted', 'Suggestion accepted')
          : action === 'reject'
            ? t('suggestions.toastRejected', 'Suggestion rejected')
            : t('suggestions.toastSnoozed', 'Snoozed'),
        'success',
      )
    } catch (e) {
      console.warn('Feedback failed:', e)
      setError(null)
      showToast(`${t('suggestions.feedbackFailed', 'Feedback failed:')} ${errorMessage(e, errorLocale)}`, 'error')
    }
  }

  const breadcrumb = targetBreadcrumb(anchor)

  if (placement === 'bottom-dock') {
    const primary = primarySuggestion
    return (
      <aside
        aria-label={t('suggestions.panelLabel', 'Suggestions panel')}
        data-placement={placement}
        data-testid="suggestion-bottom-dock"
        className={cn(
          'fixed bottom-5 left-1/2 z-panel w-[min(760px,calc(100vw-2rem))] -translate-x-1/2 rounded-xl border border-DEFAULT bg-surface-overlay px-3 py-2 text-content shadow-2xl',
          motion.transform,
          open ? 'translate-y-0 opacity-100' : 'translate-y-[calc(100%+2rem)] opacity-0',
        )}
      >
        <div className="flex items-center gap-3">
          <span className="shrink-0 rounded-full border border-muted bg-surface-muted px-2 py-1 text-content-secondary text-xs">
            {breadcrumb ?? t('suggestions.currentTarget', 'Current target')}
          </span>
          <div className="min-w-0 flex-1">
            <div className={cn('truncate text-content text-sm', typography.weight.medium)}>
              {primary?.title ?? t('suggestions.noSuggestions', 'No suggestions yet')}
            </div>
            <div className="flex items-center gap-1.5 text-[11px] text-content-tertiary">
              <Lock className={iconSize.xs} />
              <span>{t('suggestions.noAutoAction', 'No auto action')}</span>
            </div>
          </div>
          {primary && (
            <div className="flex shrink-0 items-center gap-1.5">
              <button
                type="button"
                onClick={() => void handleAction(primary.id, 'accept')}
                className={cn(
                  'rounded-md bg-brand px-2 py-1 text-content-inverse text-xs hover:bg-brand-hover',
                  motion.colors,
                )}
              >
                {t('suggestions.accept')}
              </button>
              <button
                type="button"
                onClick={() => void handleAction(primary.id, 'explain')}
                className={cn(
                  'rounded-md border border-muted bg-surface-muted px-2 py-1 text-content-secondary text-xs hover:bg-active hover:text-content',
                  motion.colors,
                )}
              >
                {t('suggestions.explain')}
              </button>
              <button
                type="button"
                onClick={() => void handleAction(primary.id, 'defer', 30)}
                className={cn('rounded-md bg-surface-muted px-2 py-1 text-content-secondary text-xs', motion.colors)}
              >
                {t('suggestions.later')}
              </button>
            </div>
          )}
        </div>
        <SuggestionReplayTrail compact anchor={anchor} suggestion={primary} />
      </aside>
    )
  }

  return (
    <aside
      aria-label={t('suggestions.panelLabel', 'Suggestions panel')}
      data-placement={placement}
      style={placementStyle(placement, anchor)}
      className={cn(
        'fixed z-panel max-h-[calc(100vh-6rem)] w-80 max-w-[calc(100vw-2rem)] transform rounded-xl border border-DEFAULT bg-surface-overlay text-content shadow-2xl',
        placement === 'window-side-panel' && anchor
          ? 'rounded-l-none border-l-DEFAULT'
          : placement === 'adjacent-popover' && anchor
            ? ''
            : 'top-20 right-4',
        motion.transform,
        open ? 'translate-x-0' : 'translate-x-[calc(100%+1rem)]',
      )}
    >
      {/* Header */}
      <div className="flex items-center justify-between border-muted border-b px-4 py-3">
        <span className={cn('text-content-secondary text-xs uppercase tracking-wider', typography.weight.semibold)}>
          {t('suggestions.panelTitle', 'Suggestions ({{count}})', { count: filteredSuggestions.length })}
        </span>
        <button
          type="button"
          onClick={onClose}
          aria-label={t('suggestions.closePanelLabel', 'Close suggestions panel')}
          className={cn('text-content-tertiary text-sm hover:text-content', motion.colors)}
        >
          &times;
        </button>
      </div>

      {breadcrumb && (
        <div className="border-muted border-b px-4 py-2">
          <span className="rounded-full border border-muted bg-surface-muted px-2 py-1 text-content-secondary text-xs">
            {breadcrumb}
          </span>
        </div>
      )}

      <SuggestionReplayTrail anchor={anchor} suggestion={primarySuggestion} />

      {/* Tab bar */}
      <div className="flex border-muted border-b">
        <button
          type="button"
          className={cn(
            'flex-1 py-2 text-xs',
            typography.weight.medium,
            motion.colors,
            activeTab === 'active'
              ? 'border-content border-b-2 text-content'
              : 'text-content-secondary hover:text-content-primary',
          )}
          onClick={() => setActiveTab('active')}
        >
          {t('suggestions.tabActive', 'Active ({{count}})', { count: filteredSuggestions.length })}
        </button>
        <button
          type="button"
          className={cn(
            'flex-1 py-2 text-xs',
            typography.weight.medium,
            motion.colors,
            activeTab === 'history'
              ? 'border-content border-b-2 text-content'
              : 'text-content-secondary hover:text-content-primary',
          )}
          onClick={() => setActiveTab('history')}
        >
          {t('suggestions.tabHistory', 'History')}
        </button>
        <button
          type="button"
          className={cn(
            'flex-1 py-2 text-xs',
            typography.weight.medium,
            motion.colors,
            activeTab === 'stats'
              ? 'border-content border-b-2 text-content'
              : 'text-content-secondary hover:text-content-primary',
          )}
          onClick={() => setActiveTab('stats')}
        >
          {t('suggestions.tabStats', 'Stats')}
        </button>
      </div>

      {/* Content */}
      <div className="max-h-[calc(100vh-14rem)] overflow-y-auto">
        {error && (
          <div className="flex items-center justify-between gap-3 border-muted border-b px-4 py-2 text-xs">
            <span className="text-semantic-error">{error}</span>
            <button
              type="button"
              aria-label={
                refreshing ? t('suggestions.retryPending', 'Refreshing...') : t('suggestions.retryLoad', 'Retry')
              }
              disabled={refreshing}
              onClick={() => void refreshSuggestions()}
              className={cn(
                'inline-flex shrink-0 items-center gap-1 rounded-md bg-brand/10 px-2 py-1 text-brand hover:bg-brand/20 disabled:cursor-not-allowed disabled:opacity-60',
                motion.colors,
              )}
            >
              {refreshing ? (
                <Loader2 className={cn(iconSize.xs, 'animate-spin')} />
              ) : (
                <RefreshCw className={iconSize.xs} />
              )}
              <span>
                {refreshing ? t('suggestions.retryPending', 'Refreshing...') : t('suggestions.retryLoad', 'Retry')}
              </span>
            </button>
          </div>
        )}
        {refreshing && !error && (
          <div className="flex items-center gap-2 border-muted border-b px-4 py-2 text-content-secondary text-xs">
            <Loader2 className={cn(iconSize.xs, 'animate-spin')} />
            <span>{t('suggestions.refreshing', 'Refreshing suggestions...')}</span>
          </div>
        )}
        {activeTab === 'active' ? (
          <>
            {/* Source filter toggles */}
            <div className="flex gap-1.5 px-3 py-1.5">
              {(['server', 'local', 'rule'] as const).map((src) => (
                <button
                  key={src}
                  type="button"
                  className={cn(
                    'rounded-full px-2 py-0.5 text-[10px]',
                    typography.weight.medium,
                    motion.colors,
                    sourceFilter.has(src)
                      ? 'border border-DEFAULT bg-surface-muted text-content-secondary'
                      : 'border border-transparent bg-transparent text-content-tertiary hover:bg-surface-muted',
                  )}
                  aria-pressed={sourceFilter.has(src)}
                  onClick={() => toggleSource(src)}
                >
                  {src === 'server'
                    ? t('suggestions.sourceServer', 'Server')
                    : src === 'local'
                      ? t('suggestions.sourceLocal', 'Local')
                      : t('suggestions.sourceRules', 'Rules')}
                </button>
              ))}
            </div>
            {filteredSuggestions.length > 0 ? (
              <ul className="list-none">
                {filteredSuggestions.map((s) => (
                  <SuggestionItem key={s.id} item={s} onAction={handleAction} onRan={refreshSuggestions} />
                ))}
              </ul>
            ) : (
              <div className="px-4 py-8 text-center text-content-tertiary text-xs">
                {t('suggestions.noSuggestions', 'No suggestions yet')}
              </div>
            )}
          </>
        ) : activeTab === 'history' ? (
          <SuggestionHistory />
        ) : (
          <SuggestionStats />
        )}
      </div>
    </aside>
  )
}
