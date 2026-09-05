import { RefreshCw } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { iconSize, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

// Aligned to the Rust SuggestionStatsDto / TypeCountDto / SourceStatsDto wire
// contract (#5699).  Fields match the Rust struct field names exactly.
interface TypeCount {
  suggestion_type: string
  count: number
}

interface SourceStats {
  source: string
  count: number
  accepted: number
  rejected: number
}

export interface LocalAnalysisStatus {
  status: 'generated' | 'no_candidate' | 'throttled' | 'provider_offline' | 'policy_blocked' | 'consent_required'
  reason: string
  producer: 'periodic' | 'app_switch'
  source: 'llm_local'
  observed_at: string
  candidate_count: number
  queue_count: number
  missing_permissions: string[]
}

// Aligned to the Rust DailyStatDto wire contract (#5699).
// Previous shape (day/total/acted/suggestion_type/source) caused a TypeError at
// day.slice(5) because `day` was undefined when rows had the new shape.
interface DailyStat {
  date: string
  shown: number
  accepted: number
  rejected: number
  deferred: number
}

interface DayAggregate {
  date: string
  total: number
  acted: number
}

// Aligned to the Rust SuggestionStatsDto fields (#5699).
// acceptance_rate is a fraction 0..1 (not a percentage integer).
interface StatsData {
  total_shown: number
  total_accepted: number
  total_rejected: number
  total_deferred: number
  acceptance_rate: number
  by_type: TypeCount[]
  by_source: SourceStats[]
  latest_local_analysis: LocalAnalysisStatus | null
}

const barColors: Record<string, string> = {
  accepted: 'bg-semantic-success',
  rejected: 'bg-semantic-error',
  deferred: 'bg-semantic-warning',
  pending: 'bg-content-secondary',
}

const localStatusDefaults: Record<LocalAnalysisStatus['status'], string> = {
  generated: 'Local suggestions generated',
  no_candidate: 'No local candidate found',
  throttled: 'Local analysis is waiting',
  provider_offline: 'Local analysis provider unavailable',
  policy_blocked: 'Local analysis is disabled',
  consent_required: 'Consent required for local analysis',
}

const localReasonDefaults: Record<string, string> = {
  generated: 'Local suggestions generated',
  no_valid_candidate: 'No valid local candidate found',
  not_admitted: 'Local candidates did not pass review filters',
  no_input: 'No local activity was available to analyze',
  context_unchanged: 'Local context has not changed',
  analysis_throttled: 'Local analysis is waiting for its next allowed run',
  provider_unavailable: 'The selected local analysis provider is unavailable',
  analysis_disabled: 'Local activity suggestion generation is disabled',
  capture_policy_blocked: 'Local analysis is blocked by capture policy',
  suggestion_queue_unavailable: 'The local suggestion review queue is unavailable',
  consent_required: 'Consent required for local analysis',
  provider_consent_required: 'The analysis provider requires consent',
  provider_policy_blocked: 'The analysis provider is blocked by policy',
}

export function LocalAnalysisStatusNotice({ status }: { status: LocalAnalysisStatus | null }) {
  const { t, i18n } = useTranslation()
  if (!status) return null

  const observedAt = new Date(status.observed_at)
  const observedLabel = Number.isNaN(observedAt.getTime())
    ? t('suggestions.localAnalysis.timeUnavailable', 'Time unavailable')
    : new Intl.DateTimeFormat(i18n.language, { dateStyle: 'short', timeStyle: 'short' }).format(observedAt)

  return (
    <div
      className="rounded-md border border-muted bg-surface-muted/70 px-3 py-2 text-[10px]"
      data-testid="local-analysis-status"
      data-status={status.status}
      data-candidate-count={status.candidate_count}
      data-queue-count={status.queue_count}
    >
      <div className={cn('text-content-primary', typography.weight.medium)}>
        {t(
          `suggestions.localAnalysis.reason.${status.reason}`,
          localReasonDefaults[status.reason] ?? localStatusDefaults[status.status],
        )}
      </div>
      <div className="mt-1 text-content-secondary">
        {t('suggestions.localAnalysis.provenance', '{{source}} · {{producer}} · {{time}}', {
          source: t('suggestions.sourceLocal', 'Local'),
          producer: t(`suggestions.localAnalysis.producer.${status.producer}`, status.producer),
          time: observedLabel,
        })}
      </div>
      {status.status === 'generated' && (
        <div className="mt-1 text-content-secondary">
          {t('suggestions.localAnalysis.counts', '{{candidateCount}} generated · {{queueCount}} in review', {
            candidateCount: status.candidate_count,
            queueCount: status.queue_count,
          })}
        </div>
      )}
    </div>
  )
}

export function SuggestionStats() {
  const { t } = useTranslation()
  const [stats, setStats] = useState<StatsData | null>(null)
  const [dailyTrends, setDailyTrends] = useState<DayAggregate[]>([])
  const [loading, setLoading] = useState(true)
  // Expose IPC failures as an explicit error state instead of hiding them behind the loading state (#4823)
  const [error, setError] = useState(false)

  // Load stats — extracted into useCallback so the retry button can reuse it
  const loadStats = useCallback(async () => {
    setLoading(true)
    setError(false)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const [data, daily] = await Promise.all([
        invoke<StatsData>('get_suggestion_stats'),
        invoke<DailyStat[]>('get_suggestion_daily_stats', { days: 7 }),
      ])
      setStats(data)
      // Aggregate daily rows by date (sum across all rows for that date).
      // Uses `date` field from the Rust DailyStatDto — previously `day` which
      // was undefined and caused the Map to collapse to one entry + slice(5) crash.
      const map = new Map<string, DayAggregate>()
      for (const row of daily) {
        const existing = map.get(row.date)
        if (existing) {
          existing.total += row.shown
          existing.acted += row.accepted
        } else {
          map.set(row.date, { date: row.date, total: row.shown, acted: row.accepted })
        }
      }
      // Sort ascending by date and take last 7
      const sorted = Array.from(map.values())
        .sort((a, b) => a.date.localeCompare(b.date))
        .slice(-7)
      setDailyTrends(sorted)
    } catch (e) {
      console.warn('Failed to load stats:', e)
      setError(true)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadStats()
  }, [loadStats])

  if (loading) return <p className="p-4 text-content-secondary text-xs">{t('common.loading', 'Loading...')}</p>
  // On error, use the same error/retry banner pattern as SuggestionsPanel
  if (error) {
    return (
      <div className="flex items-center justify-between gap-3 p-4 text-xs">
        <span className="text-semantic-error">{t('suggestionStats.loadError', 'Could not load stats.')}</span>
        <button
          type="button"
          aria-label={t('suggestions.retryLoad', 'Retry')}
          onClick={() => void loadStats()}
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
  if (!stats) return null
  if (stats.total_shown === 0)
    return (
      <div className="flex flex-col gap-3 p-3">
        <LocalAnalysisStatusNotice status={stats.latest_local_analysis} />
        <p className="text-content-secondary text-xs">{t('suggestionStats.noData', 'No data yet')}</p>
      </div>
    )

  // Derive pending = total_shown minus all explicitly-labelled outcomes.
  const pending = stats.total_shown - (stats.total_accepted + stats.total_rejected + stats.total_deferred)

  const entries = [
    { key: 'accepted', label: t('suggestionStats.accepted', 'Accepted'), count: stats.total_accepted },
    { key: 'rejected', label: t('suggestionStats.rejected', 'Rejected'), count: stats.total_rejected },
    { key: 'deferred', label: t('suggestionStats.snoozed', 'Snoozed'), count: stats.total_deferred },
    { key: 'pending', label: t('suggestionStats.pending', 'Pending'), count: pending },
  ]

  return (
    <div className="flex flex-col gap-3 p-3">
      <LocalAnalysisStatusNotice status={stats.latest_local_analysis} />
      <div className="text-center">
        {/* acceptance_rate is a fraction 0..1 — render as percentage (#5699) */}
        <div className={cn('text-2xl text-brand', typography.weight.bold)}>
          {Math.round(stats.acceptance_rate * 100)}%
        </div>
        <div className="text-[10px] text-content-secondary">
          {t('suggestionStats.acceptanceRate', 'Acceptance Rate')}
        </div>
      </div>
      <div className="text-center text-[10px] text-content-secondary">
        {t('suggestionStats.totalSuggestions', '{{count}} total suggestions', { count: stats.total_shown })}
      </div>
      <div className="flex flex-col gap-1.5">
        {entries.map(({ key, label, count }) => (
          <div key={key} className="flex items-center gap-2">
            <span className="w-14 text-[10px] text-content-secondary">{label}</span>
            <div className="h-3 flex-1 overflow-hidden rounded-full bg-content-inverse/5">
              <div
                className={cn('h-full rounded-full', motion.all, barColors[key])}
                style={{ width: `${stats.total_shown > 0 ? (count / stats.total_shown) * 100 : 0}%` }}
              />
            </div>
            <span className="w-6 text-right text-[10px] text-content-primary">{count}</span>
          </div>
        ))}
      </div>

      {/* Type Distribution */}
      {stats.by_type.length > 0 && (
        <>
          <div className={cn('pt-1 text-[10px] text-content-secondary', typography.weight.medium)}>
            {t('suggestionStats.typeDistribution', 'Type Distribution')}
          </div>
          <div className="flex flex-col gap-1">
            {stats.by_type.map(({ suggestion_type, count }) => {
              const maxCount = stats.by_type[0]?.count ?? 1
              return (
                <div key={suggestion_type} className="flex items-center gap-2">
                  <span className="w-24 truncate text-[10px] text-content-secondary" title={suggestion_type}>
                    {suggestion_type}
                  </span>
                  <div className="h-2.5 flex-1 overflow-hidden rounded-full bg-content-inverse/5">
                    <div
                      className={cn('h-full rounded-full bg-brand/60', motion.all)}
                      style={{ width: `${maxCount > 0 ? (count / maxCount) * 100 : 0}%` }}
                    />
                  </div>
                  <span className="w-6 text-right text-[10px] text-content-primary">{count}</span>
                </div>
              )
            })}
          </div>
        </>
      )}

      {/* Source Quality — acceptance_rate derived from accepted/count (#5699) */}
      {stats.by_source.length > 0 && (
        <>
          <div className={cn('pt-1 text-[10px] text-content-secondary', typography.weight.medium)}>
            {t('suggestionStats.sourceQuality', 'Source Quality')}
          </div>
          <div className="flex flex-col gap-1">
            {stats.by_source.map(({ source, count, accepted }) => {
              const acceptance_rate = count > 0 ? Math.round((accepted / count) * 100) : 0
              return (
                <div key={source} className="flex items-center justify-between">
                  <span className="w-20 truncate text-[10px] text-content-secondary" title={source}>
                    {source}
                  </span>
                  <span className="text-[10px] text-content-primary">
                    {t('suggestionStats.countTotal', '{{count}} total', { count })}
                  </span>
                  <span
                    className={cn(
                      'w-12 text-right text-[10px]',
                      typography.weight.medium,
                      acceptance_rate >= 50 ? 'text-semantic-success' : 'text-content-secondary',
                    )}
                  >
                    {acceptance_rate}%
                  </span>
                </div>
              )
            })}
          </div>
        </>
      )}

      {/* Daily Trends — uses `date` field (was `day`), `total` from shown, `acted` from accepted (#5699) */}
      {dailyTrends.length > 0 &&
        (() => {
          const maxTotal = Math.max(...dailyTrends.map((d) => d.total), 1)
          return (
            <>
              <div className={cn('pt-1 text-[10px] text-content-secondary', typography.weight.medium)}>
                {t('suggestionStats.dailyTrends', 'Daily Trends (7d)')}
              </div>
              <div className="flex flex-col gap-1">
                {dailyTrends.map(({ date, total, acted }) => (
                  <div key={date} className="flex items-center gap-2">
                    <span className="w-14 text-[10px] text-content-secondary tabular-nums">{date.slice(5)}</span>
                    <div className="relative h-3 flex-1 overflow-hidden rounded-full bg-content-inverse/5">
                      <div
                        className={cn('absolute inset-y-0 left-0 rounded-full bg-brand/30', motion.all)}
                        style={{ width: `${(total / maxTotal) * 100}%` }}
                      />
                      <div
                        className={cn('absolute inset-y-0 left-0 rounded-full bg-brand', motion.all)}
                        style={{ width: `${(acted / maxTotal) * 100}%` }}
                      />
                    </div>
                    <span className="w-10 text-right text-[10px] text-content-primary tabular-nums">
                      {acted}/{total}
                    </span>
                  </div>
                ))}
              </div>
              <div className="flex items-center justify-center gap-3 text-[9px] text-content-secondary">
                <span className="flex items-center gap-1">
                  <span className="inline-block h-2 w-2 rounded-full bg-brand" />
                  {t('suggestionStats.acted', 'Acted')}
                </span>
                <span className="flex items-center gap-1">
                  <span className="inline-block h-2 w-2 rounded-full bg-brand/30" />
                  {t('suggestionStats.total', 'Total')}
                </span>
              </div>
            </>
          )
        })()}
    </div>
  )
}
