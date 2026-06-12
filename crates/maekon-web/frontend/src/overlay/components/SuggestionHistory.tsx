import { RefreshCw } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { iconSize, motion, typography } from '../../styles/tokens'
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
  // IPC 실패를 빈 상태로 숨기지 않고 명시적 에러 상태로 노출 (#4823)
  const [error, setError] = useState(false)

  // 히스토리 로드 — 재시도 버튼에서도 재사용하기 위해 useCallback 으로 분리
  const loadHistory = useCallback(async () => {
    setLoading(true)
    setError(false)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const result = await invoke<SuggestionHistoryDto[]>('get_suggestion_history', { limit: 50 })
      setEntries(result)
    } catch (e) {
      console.warn('Failed to load history:', e)
      setError(true)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadHistory()
  }, [loadHistory])

  if (loading) {
    return <p className="p-4 text-content-secondary text-xs">{t('common.loading', 'Loading...')}</p>
  }

  // 에러 시 SuggestionsPanel 과 동일한 에러/재시도 배너 패턴 사용
  if (error) {
    return (
      <div className="flex items-center justify-between gap-3 p-4 text-xs">
        <span className="text-semantic-error">{t('suggestions.historyLoadError', 'Could not load history.')}</span>
        <button
          type="button"
          aria-label={t('suggestions.retryLoad', 'Retry')}
          onClick={() => void loadHistory()}
          className={cn(
            'inline-flex shrink-0 items-center gap-1 rounded-md bg-brand/10 px-2 py-1 text-brand hover:bg-brand/20',
            motion.colors,
          )}
        >
          <RefreshCw className={iconSize.xs} />
          <span>{t('suggestions.retryLoad', 'Retry')}</span>
        </button>
      </div>
    )
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
