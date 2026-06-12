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
}

const barColors: Record<string, string> = {
  accepted: 'bg-semantic-success',
  rejected: 'bg-semantic-error',
  deferred: 'bg-semantic-warning',
  pending: 'bg-content-secondary',
}

export function SuggestionStats() {
  const { t } = useTranslation()
  const [stats, setStats] = useState<StatsData | null>(null)
  const [dailyTrends, setDailyTrends] = useState<DayAggregate[]>([])
  const [loading, setLoading] = useState(true)
  // IPC 실패를 로딩 상태로 숨기지 않고 명시적 에러 상태로 노출 (#4823)
  const [error, setError] = useState(false)

  // 통계 로드 — 재시도 버튼에서도 재사용하기 위해 useCallback 으로 분리
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
  // 에러 시 SuggestionsPanel 과 동일한 에러/재시도 배너 패턴 사용
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
  if (!stats || stats.total_shown === 0)
    return <p className="p-4 text-content-secondary text-xs">{t('suggestionStats.noData', 'No data yet')}</p>

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
