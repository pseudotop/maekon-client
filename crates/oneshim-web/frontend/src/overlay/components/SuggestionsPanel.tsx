import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { SuggestionViewDto } from '../types'
import { SuggestionHistory } from './SuggestionHistory'
import { SuggestionItem } from './SuggestionItem'
import { SuggestionStats } from './SuggestionStats'
import { showToast } from './Toast'

function errorMessage(e: unknown): string {
  if (e instanceof Error && e.message.trim()) return e.message
  if (typeof e === 'string' && e.trim()) return e
  return 'Unknown error'
}

interface SuggestionsPanelProps {
  open: boolean
  suggestions: SuggestionViewDto[]
  onClose: () => void
  onRefresh: () => Promise<void> | void
}

/**
 * Focus management: Overlay panels run in compact Tauri WebView windows.
 * - Escape key dismisses the panel (global keydown listener).
 * - No focus trap needed — the panel IS the entire window content.
 * - Interactive elements use standard tab order within the panel.
 */
export function SuggestionsPanel({ open, suggestions, onClose, onRefresh }: SuggestionsPanelProps) {
  const { t } = useTranslation()
  const [error, setError] = useState<string | null>(null)
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

  const filteredSuggestions = useMemo(
    () => suggestions.filter((s) => sourceFilter.has(s.source)),
    [suggestions, sourceFilter],
  )

  const toggleSource = (source: string) => {
    setSourceFilter((prev) => {
      const next = new Set(prev)
      if (next.has(source)) next.delete(source)
      else next.add(source)
      localStorage.setItem('suggestion-source-filter', JSON.stringify([...next]))
      return next
    })
  }

  useEffect(() => {
    if (!open) return

    setError(null)
    void Promise.resolve(onRefresh()).catch((e) => {
      console.warn('SuggestionsPanel refresh failed:', e)
      setError(t('suggestions.loadError', 'Could not load suggestions.'))
    })
  }, [open, onRefresh, t])

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape' && open) onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  async function handleAction(id: string, action: 'accept' | 'reject' | 'defer' | 'explain', snoozeMinutes?: number) {
    if (action === 'explain') {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('explain_suggestion_in_chat', { suggestionId: id })
        showToast(t('suggestions.openingInChat', 'Opening in chat...'), 'info')
      } catch (e) {
        showToast(errorMessage(e), 'error')
      }
      return
    }
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      // Tauri v2 auto-converts camelCase JS -> snake_case Rust params
      await invoke('submit_suggestion_feedback', { suggestionId: id, action, snoozeMinutes })
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
      showToast(`${t('suggestions.feedbackFailed', 'Feedback failed:')} ${errorMessage(e)}`, 'error')
    }
  }

  return (
    <aside
      aria-label={t('suggestions.panelLabel', 'Suggestions panel')}
      className={cn(
        'fixed top-20 right-4 z-panel max-h-[calc(100vh-6rem)] w-80 max-w-[calc(100vw-2rem)] transform rounded-xl border border-content-inverse/10 bg-surface-sunken/90 shadow-2xl backdrop-blur-md',
        motion.transform,
        open ? 'translate-x-0' : 'translate-x-[calc(100%+1rem)]',
      )}
    >
      {/* Header */}
      <div className="flex items-center justify-between border-content-inverse/5 border-b px-4 py-3">
        <span className={cn('text-content-secondary text-xs uppercase tracking-wider', typography.weight.semibold)}>
          {t('suggestions.panelTitle', 'Suggestions ({{count}})', { count: suggestions.length })}
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

      {/* Tab bar */}
      <div className="flex border-content-inverse/5 border-b">
        <button
          type="button"
          className={cn(
            'flex-1 py-2 text-xs',
            typography.weight.medium,
            motion.colors,
            activeTab === 'active'
              ? 'border-brand border-b-2 text-brand'
              : 'text-content-secondary hover:text-content-primary',
          )}
          onClick={() => setActiveTab('active')}
        >
          {t('suggestions.tabActive', 'Active ({{count}})', { count: suggestions.length })}
        </button>
        <button
          type="button"
          className={cn(
            'flex-1 py-2 text-xs',
            typography.weight.medium,
            motion.colors,
            activeTab === 'history'
              ? 'border-brand border-b-2 text-brand'
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
              ? 'border-brand border-b-2 text-brand'
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
          <div className="border-content-inverse/5 border-b px-4 py-2 text-semantic-error text-xs">{error}</div>
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
                    sourceFilter.has(src) ? 'bg-brand/20 text-brand' : 'bg-content-inverse/5 text-content-tertiary',
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
                  <SuggestionItem key={s.id} item={s} onAction={handleAction} />
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
