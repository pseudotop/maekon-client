/**
 * Today at a Glance — summary widget for the Dashboard.
 * Shows active time, top app, coaching nudge count, and current regime.
 */

import { useQuery } from '@tanstack/react-query'
import { Activity, Brain, Clock, Target } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { fetchCoachingStatsToday } from '../api/client'
import { colors, iconSize, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { formatDuration } from '../utils/formatters'
import { Card, CardTitle } from './ui'

export interface TodaySummaryProps {
  totalActiveSecs: number
  topApps: Array<{ name: string }>
}

export default function TodaySummary({ totalActiveSecs, topApps }: TodaySummaryProps) {
  const { t } = useTranslation()

  const { data: coachingStats } = useQuery({
    queryKey: ['coachingStatsToday'],
    queryFn: fetchCoachingStatsToday,
    refetchInterval: 60_000,
  })

  const topAppName = topApps.length > 0 ? topApps[0].name : '-'

  const items = [
    {
      icon: <Clock className={iconSize.md} />,
      label: t('dashboard.todaySummary.activeTime'),
      value: formatDuration(totalActiveSecs, true),
    },
    {
      icon: <Activity className={iconSize.md} />,
      label: t('dashboard.todaySummary.topApp'),
      value: topAppName,
    },
    {
      icon: <Brain className={iconSize.md} />,
      label: t('dashboard.todaySummary.nudges'),
      value: String(coachingStats?.nudges_count ?? 0),
    },
    {
      icon: <Target className={iconSize.md} />,
      label: t('dashboard.todaySummary.regime'),
      value: coachingStats?.current_regime ?? '-',
    },
  ]

  return (
    <Card variant="default" padding="md" data-testid="today-summary">
      <CardTitle className="mb-3">{t('dashboard.todaySummary.title')}</CardTitle>
      <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
        {items.map((item) => (
          <div key={item.label} className="flex items-center space-x-3">
            <div className={cn('shrink-0 rounded-lg bg-brand-signal/10 p-2 text-brand-text')}>{item.icon}</div>
            <div className="min-w-0">
              <div className={cn(typography.stat.normal, colors.text.primary, 'truncate')}>{item.value}</div>
              <div className={cn(typography.caption, colors.text.secondary)}>{item.label}</div>
            </div>
          </div>
        ))}
      </div>
    </Card>
  )
}
