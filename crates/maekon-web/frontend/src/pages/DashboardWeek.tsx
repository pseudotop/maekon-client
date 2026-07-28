/**
 * DashboardWeek — weekly productivity digest view (#5676).
 *
 * Surfaces the weekly digest that the scheduler has been generating and
 * persisting all along (generator → SQLite → REST → client fns were all
 * built); this page is the previously-missing consumer. Numbers in the
 * regime/category breakdowns are HOURS (f32), not percentages.
 */
import { useQuery } from '@tanstack/react-query'
import { CalendarRange, MessageSquareOff, TrendingDown, TrendingUp } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { fetchCurrentDigest, fetchSummary } from '../api/client'
import type { DailySummary, WeeklyDigest } from '../api/contracts'
import CaptureProcessingStatus, { hasCapturedActivity } from '../components/CaptureProcessingStatus'
import { Card, CardContent, CardTitle, Skeleton } from '../components/ui'
import { colors, iconSize, typography } from '../styles/tokens'
import { cn } from '../utils/cn'

/** "ACTIVE_CODING" (wire WorkType) → "Active Coding". */
function humanizeWorkType(workType: string): string {
  return workType
    .toLowerCase()
    .split('_')
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(' ')
}

function formatRange(startIso: string, endIso: string): string {
  try {
    const opts: Intl.DateTimeFormatOptions = { month: 'short', day: 'numeric' }
    const start = new Date(startIso).toLocaleDateString(undefined, opts)
    const end = new Date(endIso).toLocaleDateString(undefined, opts)
    return `${start} – ${end}`
  } catch {
    return `${startIso} – ${endIso}`
  }
}

/** `/digests/current` returns the NEWEST row, which may be last week's —
 *  surface that instead of implying it is always "this week". */
function isStale(digest: WeeklyDigest): boolean {
  const end = new Date(digest.week_end).getTime()
  return Number.isFinite(end) && Date.now() - end > 7 * 24 * 3600 * 1000
}

function StatTile({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-border-default p-3">
      <p className={cn('text-xs', colors.text.secondary)}>{label}</p>
      <p className={cn('text-lg', typography.weight.bold)}>{value}</p>
    </div>
  )
}

function HoursBarList({ title, breakdown }: { title: string; breakdown: Record<string, number> }) {
  const entries = Object.entries(breakdown).sort((a, b) => b[1] - a[1])
  if (entries.length === 0) return null
  const max = Math.max(...entries.map(([, h]) => h), 0.1)
  return (
    <Card variant="default" padding="md">
      <CardTitle>{title}</CardTitle>
      <CardContent>
        <ul className="space-y-2">
          {entries.map(([label, hours]) => (
            <li key={label} className="flex items-center gap-2">
              <span className={cn('w-32 truncate text-sm', colors.text.secondary)}>{label}</span>
              <div className="h-2 flex-1 rounded bg-surface-muted">
                <div
                  className="h-2 rounded bg-accent-primary"
                  style={{ width: `${Math.min(100, (hours / max) * 100)}%` }}
                />
              </div>
              <span className={cn('w-14 text-right text-sm', typography.weight.medium)}>{hours.toFixed(1)}h</span>
            </li>
          ))}
        </ul>
      </CardContent>
    </Card>
  )
}

function DeltaRow({ label, deltaHours }: { label: string; deltaHours: number }) {
  const positive = deltaHours >= 0
  const Icon = positive ? TrendingUp : TrendingDown
  return (
    <li className="flex items-center gap-2 text-sm">
      <Icon className={cn(iconSize.sm, positive ? 'text-semantic-success' : 'text-semantic-error')} />
      <span className={colors.text.secondary}>{label}</span>
      <span className={typography.weight.medium}>
        {positive ? '+' : ''}
        {deltaHours.toFixed(1)}h
      </span>
    </li>
  )
}

export default function DashboardWeek() {
  const { t } = useTranslation()
  const { data: digest, isLoading } = useQuery<WeeklyDigest | null>({
    queryKey: ['dashboard-week-current'],
    queryFn: fetchCurrentDigest,
  })
  const { data: rawTodaySummary } = useQuery<DailySummary>({
    queryKey: ['dashboard-week-raw-today-summary'],
    queryFn: () => fetchSummary(),
  })

  if (isLoading) {
    return (
      <div className="space-y-4 p-4">
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-40 w-full" />
      </div>
    )
  }

  if (!digest) {
    return (
      <div className="p-4">
        <Card variant="default" padding="md">
          <CardTitle>{t('week.title', 'Weekly Digest')}</CardTitle>
          <CardContent>
            <p className={cn('text-sm', colors.text.secondary)}>
              {t(
                'week.empty',
                'No weekly digest yet. One is generated automatically each week once activity segments accumulate.',
              )}
            </p>
          </CardContent>
        </Card>
      </div>
    )
  }

  const rawTrackedHours = (rawTodaySummary?.total_active_secs ?? 0) / 3600
  const trackedHours = Math.max(digest.total_tracked_hours, rawTrackedHours)
  const analysisPending = digest.total_tracked_hours === 0 && hasCapturedActivity(rawTodaySummary)

  return (
    <div className="space-y-4 p-4">
      <div className="flex items-center gap-2">
        <CalendarRange className={iconSize.base} />
        <h1 className={cn('text-xl', typography.weight.bold)}>{t('week.title', 'Weekly Digest')}</h1>
        <span className={cn('text-sm', colors.text.secondary)}>{formatRange(digest.week_start, digest.week_end)}</span>
        {isStale(digest) && (
          <span className="rounded bg-surface-muted px-2 py-0.5 text-xs">{t('week.stale', 'Last available week')}</span>
        )}
      </div>

      {analysisPending && <CaptureProcessingStatus summary={rawTodaySummary} />}

      <div className="grid grid-cols-2 gap-3 md:grid-cols-5">
        <StatTile label={t('week.totalTracked', 'Tracked')} value={`${trackedHours.toFixed(1)}h`} />
        <StatTile label={t('week.deepWork', 'Deep work')} value={`${digest.deep_work_hours.toFixed(1)}h`} />
        <StatTile
          label={t('week.communication', 'Communication')}
          value={`${digest.communication_hours.toFixed(1)}h`}
        />
        <StatTile label={t('week.contextSwitches', 'Context switches')} value={String(digest.context_switches_total)} />
        <StatTile
          label={t('week.longestSegment', 'Longest focus')}
          value={`${digest.longest_deep_work_segment_mins}m`}
        />
      </div>

      {digest.comparison && (
        <Card variant="default" padding="md">
          <CardTitle>{t('week.comparison', 'vs. previous week')}</CardTitle>
          <CardContent>
            <ul className="space-y-1">
              <DeltaRow label={t('week.deepWork', 'Deep work')} deltaHours={digest.comparison.deep_work_delta_hours} />
              <DeltaRow
                label={t('week.communication', 'Communication')}
                deltaHours={digest.comparison.communication_delta_hours}
              />
              <li className={cn('pt-1 text-sm', colors.text.secondary)}>{digest.comparison.trend_summary}</li>
            </ul>
          </CardContent>
        </Card>
      )}

      <div className="grid gap-4 md:grid-cols-2">
        <HoursBarList title={t('week.regimes', 'Regimes (hours)')} breakdown={digest.regime_breakdown} />
        <HoursBarList title={t('week.categories', 'Categories (hours)')} breakdown={digest.category_breakdown} />
      </div>

      {digest.top_content.length > 0 && (
        <Card variant="default" padding="md">
          <CardTitle>{t('week.topContent', 'Top content')}</CardTitle>
          <CardContent>
            <ul className="space-y-1">
              {digest.top_content.map((item) => (
                <li key={item.content_label} className="flex items-center gap-2 text-sm">
                  <span className="flex-1 truncate">{item.content_label}</span>
                  <span className={cn('text-xs', colors.text.secondary)}>
                    {humanizeWorkType(item.dominant_work_type)}
                  </span>
                  <span className={typography.weight.medium}>{item.total_mins}m</span>
                </li>
              ))}
            </ul>
          </CardContent>
        </Card>
      )}

      <Card variant="default" padding="md">
        <CardTitle>{t('week.narrative', 'Weekly narrative')}</CardTitle>
        <CardContent>
          {digest.llm_narrative ? (
            <p className="text-sm">{digest.llm_narrative}</p>
          ) : (
            <p className={cn('flex items-center gap-2 text-sm', colors.text.secondary)}>
              <MessageSquareOff className={iconSize.sm} />
              {t('week.narrativeUnavailable', 'Narrative unavailable.')}
            </p>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
