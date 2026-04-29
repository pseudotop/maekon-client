import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { SuggestionHistoryDto } from '../types'

const feedbackBadgeClassName: Record<string, string> = {
  accepted: 'bg-semantic-success/20 text-semantic-success',
  rejected: 'bg-semantic-error/20 text-semantic-error',
  deferred: 'bg-semantic-warning/20 text-semantic-warning',
}

const feedbackBadgeKey: Record<string, string> = {
  accepted: 'suggestions.feedbackAccepted',
  rejected: 'suggestions.feedbackRejected',
  deferred: 'suggestions.feedbackSnoozed',
}

export function SuggestionHistory() {
  const { t } = useTranslation()
  const [entries, setEntries] = useState<SuggestionHistoryDto[]>([])
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    let cancelled = false
    ;(async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        const result = await invoke<SuggestionHistoryDto[]>('get_suggestion_history', { limit: 50 })
        if (!cancelled) setEntries(result)
      } catch (e) {
        console.warn('Failed to load history:', e)
      } finally {
        if (!cancelled) setLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  if (loading) {
    return <p className="p-4 text-content-secondary text-xs">{t('common.loading', 'Loading...')}</p>
  }

  if (entries.length === 0) {
    return <p className="p-4 text-content-secondary text-xs">{t('suggestions.noHistory', 'No history yet')}</p>
  }

  const stats = {
    accepted: entries.filter((e) => e.feedback === 'accepted').length,
    rejected: entries.filter((e) => e.feedback === 'rejected').length,
    deferred: entries.filter((e) => e.feedback === 'deferred').length,
    pending: entries.filter((e) => !e.feedback).length,
  }

  return (
    <div className="flex flex-col gap-2 p-2">
      <div className="flex gap-3 border-border-default border-b px-2 pb-2 text-content-secondary text-xs">
        <span>
          {stats.accepted} {t('suggestions.statsAccepted', 'accepted')}
        </span>
        <span>
          {stats.rejected} {t('suggestions.statsRejected', 'rejected')}
        </span>
        <span>
          {stats.deferred} {t('suggestions.statsSnoozed', 'snoozed')}
        </span>
        <span>
          {stats.pending} {t('suggestions.statsPending', 'pending')}
        </span>
      </div>
      <ul className="flex flex-col gap-1.5">
        {entries.map((entry) => {
          const badgeClass = entry.feedback ? feedbackBadgeClassName[entry.feedback] : null
          const badgeKey = entry.feedback ? feedbackBadgeKey[entry.feedback] : null
          return (
            <li key={entry.id} className="rounded-lg bg-surface-default/60 px-3 py-2 text-xs">
              <div className="flex items-center justify-between gap-2">
                <span className={cn(typography.weight.medium, 'truncate text-content-primary')}>{entry.title}</span>
                {badgeClass && badgeKey && (
                  <span
                    className={cn('shrink-0 rounded px-1.5 py-0.5 text-[10px]', typography.weight.medium, badgeClass)}
                  >
                    {t(badgeKey)}
                  </span>
                )}
              </div>
              <p className="mt-0.5 line-clamp-1 text-content-secondary">{entry.body}</p>
            </li>
          )
        })}
      </ul>
    </div>
  )
}
