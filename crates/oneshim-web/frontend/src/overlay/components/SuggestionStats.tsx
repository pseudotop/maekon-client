import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface TypeCount {
  suggestion_type: string
  count: number
}

interface SourceStats {
  source: string
  count: number
  acceptance_rate: number
}

interface DailyStat {
  day: string
  total: number
  acted: number
  suggestion_type: string
  source: string
}

interface DayAggregate {
  day: string
  total: number
  acted: number
}

interface StatsData {
  total: number
  accepted: number
  rejected: number
  deferred: number
  pending: number
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

  useEffect(() => {
    let cancelled = false
    ;(async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        const [data, daily] = await Promise.all([
          invoke<StatsData>('get_suggestion_stats'),
          invoke<DailyStat[]>('get_suggestion_daily_stats', { days: 7 }),
        ])
        if (cancelled) return
        setStats(data)
        // Aggregate daily rows by day (sum across types/sources)
        const map = new Map<string, DayAggregate>()
        for (const row of daily) {
          const existing = map.get(row.day)
          if (existing) {
            existing.total += row.total
            existing.acted += row.acted
          } else {
            map.set(row.day, { day: row.day, total: row.total, acted: row.acted })
          }
        }
        // Sort ascending by day and take last 7
        const sorted = Array.from(map.values())
          .sort((a, b) => a.day.localeCompare(b.day))
          .slice(-7)
        setDailyTrends(sorted)
      } catch (e) {
        console.warn('Failed to load stats:', e)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  if (!stats) return <p className="p-4 text-content-secondary text-xs">{t('common.loading', 'Loading...')}</p>
  if (stats.total === 0)
    return <p className="p-4 text-content-secondary text-xs">{t('suggestionStats.noData', 'No data yet')}</p>

  const entries = [
    { key: 'accepted', label: t('suggestionStats.accepted', 'Accepted'), count: stats.accepted },
    { key: 'rejected', label: t('suggestionStats.rejected', 'Rejected'), count: stats.rejected },
    { key: 'deferred', label: t('suggestionStats.snoozed', 'Snoozed'), count: stats.deferred },
    { key: 'pending', label: t('suggestionStats.pending', 'Pending'), count: stats.pending },
  ]

  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="text-center">
        <div className={cn('text-2xl text-brand', typography.weight.bold)}>{stats.acceptance_rate}%</div>
        <div className="text-[10px] text-content-secondary">
          {t('suggestionStats.acceptanceRate', 'Acceptance Rate')}
        </div>
      </div>
      <div className="text-center text-[10px] text-content-secondary">
        {t('suggestionStats.totalSuggestions', '{{count}} total suggestions', { count: stats.total })}
      </div>
      <div className="flex flex-col gap-1.5">
        {entries.map(({ key, label, count }) => (
          <div key={key} className="flex items-center gap-2">
            <span className="w-14 text-[10px] text-content-secondary">{label}</span>
            <div className="h-3 flex-1 overflow-hidden rounded-full bg-content-inverse/5">
              <div
                className={cn('h-full rounded-full', motion.all, barColors[key])}
                style={{ width: `${stats.total > 0 ? (count / stats.total) * 100 : 0}%` }}
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

      {/* Source Quality */}
      {stats.by_source.length > 0 && (
        <>
          <div className={cn('pt-1 text-[10px] text-content-secondary', typography.weight.medium)}>
            {t('suggestionStats.sourceQuality', 'Source Quality')}
          </div>
          <div className="flex flex-col gap-1">
            {stats.by_source.map(({ source, count, acceptance_rate }) => (
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
            ))}
          </div>
        </>
      )}

      {/* Daily Trends */}
      {dailyTrends.length > 0 &&
        (() => {
          const maxTotal = Math.max(...dailyTrends.map((d) => d.total), 1)
          return (
            <>
              <div className={cn('pt-1 text-[10px] text-content-secondary', typography.weight.medium)}>
                {t('suggestionStats.dailyTrends', 'Daily Trends (7d)')}
              </div>
              <div className="flex flex-col gap-1">
                {dailyTrends.map(({ day, total, acted }) => (
                  <div key={day} className="flex items-center gap-2">
                    <span className="w-14 text-[10px] text-content-secondary tabular-nums">{day.slice(5)}</span>
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
