/**
 * HabitTrackerWidget -- 7-day habit streak grid per regime.
 * Cells are green (met), yellow (>50%), or red (<50%).
 * Shows the current consecutive streak count per regime.
 */

import { Flame } from 'lucide-react'
import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import type { HabitStreak } from '../api/coaching'
import { useHabitStreaks } from '../hooks/useCoaching'
import { colors, iconSize, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { Card, CardContent, CardTitle, Skeleton } from './ui'

/** Build a map of `regime -> date -> HabitStreak` for quick lookups. */
function buildStreakMap(data: HabitStreak[]) {
  const map = new Map<string, Map<string, HabitStreak>>()
  for (const row of data) {
    let dateMap = map.get(row.regime_label)
    if (!dateMap) {
      dateMap = new Map()
      map.set(row.regime_label, dateMap)
    }
    dateMap.set(row.date, row)
  }
  return map
}

/** Get the last N days as YYYY-MM-DD strings, most recent first. */
function lastNDays(n: number): string[] {
  const days: string[] = []
  const now = new Date()
  for (let i = 0; i < n; i++) {
    const d = new Date(now)
    d.setDate(d.getDate() - i)
    days.push(d.toISOString().slice(0, 10))
  }
  return days
}

/** Compute the current consecutive streak (met days from today backwards). */
function computeStreak(dateMap: Map<string, HabitStreak>, days: string[]): number {
  let streak = 0
  for (const date of days) {
    const entry = dateMap.get(date)
    if (entry?.met) {
      streak++
    } else {
      break
    }
  }
  return streak
}

/** Return a Tailwind background class based on progress ratio. */
function cellBg(entry: HabitStreak | undefined): string {
  if (!entry) return 'bg-surface-muted'
  if (entry.met) return 'bg-semantic-success'
  const ratio = entry.target_minutes > 0 ? entry.minutes_logged / entry.target_minutes : 0
  if (ratio >= 0.5) return 'bg-semantic-warning'
  return 'bg-semantic-error'
}

/** Short day label (Mon, Tue, ...) from YYYY-MM-DD string. */
function shortDay(dateStr: string): string {
  const d = new Date(`${dateStr}T00:00:00`)
  return d.toLocaleDateString(undefined, { weekday: 'short' })
}

export default function HabitTrackerWidget() {
  const { t } = useTranslation()
  const { data, isLoading } = useHabitStreaks(7)

  const days = useMemo(() => lastNDays(7), [])

  const streakMap = useMemo(() => buildStreakMap(data ?? []), [data])

  const regimes = useMemo(() => Array.from(streakMap.keys()).sort(), [streakMap])

  if (isLoading) {
    return (
      <Card variant="default" padding="md">
        <Skeleton className="h-32 w-full" />
      </Card>
    )
  }

  if (regimes.length === 0) {
    return (
      <Card variant="default" padding="md">
        <CardTitle>{t('coaching.habits.title', 'Habit Tracker')}</CardTitle>
        <CardContent>
          <p className={cn('text-sm', colors.text.secondary)}>
            {t('coaching.habits.empty', 'No habit data yet. Streaks will appear once goals are tracked.')}
          </p>
        </CardContent>
      </Card>
    )
  }

  return (
    <Card variant="default" padding="md">
      <CardTitle>{t('coaching.habits.title', 'Habit Tracker')}</CardTitle>
      <CardContent>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr>
                <th className={cn('pb-2 text-left', typography.weight.medium, colors.text.secondary)}>
                  {t('coaching.habits.regime', 'Regime')}
                </th>
                {days.map((d) => (
                  <th key={d} className={cn('pb-2 text-center', typography.weight.medium, colors.text.secondary)}>
                    {shortDay(d)}
                  </th>
                ))}
                <th className={cn('pb-2 text-center', typography.weight.medium, colors.text.secondary)}>
                  <Flame className={cn('mx-auto', iconSize.base)} />
                </th>
              </tr>
            </thead>
            <tbody>
              {regimes.map((regime) => {
                const dateMap = streakMap.get(regime) ?? new Map<string, HabitStreak>()
                const streak = computeStreak(dateMap, days)
                return (
                  <tr key={regime}>
                    <td className={cn('max-w-[120px] truncate py-1 pr-3 text-left', typography.weight.medium)}>
                      {regime}
                    </td>
                    {days.map((d) => {
                      const entry = dateMap.get(d)
                      const pct =
                        entry && entry.target_minutes > 0
                          ? Math.round((entry.minutes_logged / entry.target_minutes) * 100)
                          : 0
                      const tooltip = entry
                        ? `${entry.minutes_logged}/${entry.target_minutes}m (${pct}%)`
                        : t('coaching.habits.noData', 'No data')
                      return (
                        <td key={d} className="px-1 py-1 text-center">
                          <div className={cn('mx-auto h-6 w-6 rounded', cellBg(entry))} title={tooltip} />
                        </td>
                      )
                    })}
                    <td className={cn('py-1 text-center', typography.weight.bold)}>{streak > 0 ? streak : '-'}</td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
        <div className="mt-3 flex items-center gap-4 text-xs">
          <span className="flex items-center gap-1">
            <span className={cn('inline-block rounded bg-semantic-success', iconSize.xs)} />
            {t('coaching.habits.met', 'Met')}
          </span>
          <span className="flex items-center gap-1">
            <span className={cn('inline-block rounded bg-semantic-warning', iconSize.xs)} />
            {t('coaching.habits.partial', '>50%')}
          </span>
          <span className="flex items-center gap-1">
            <span className={cn('inline-block rounded bg-semantic-error', iconSize.xs)} />
            {t('coaching.habits.missed', '<50%')}
          </span>
          <span className="flex items-center gap-1">
            <span className={cn('inline-block rounded bg-surface-muted', iconSize.xs)} />
            {t('coaching.habits.noData', 'No data')}
          </span>
        </div>
      </CardContent>
    </Card>
  )
}
