/**
 * Overview section — realtime metrics card, TodaySummary, and stat cards.
 *
 * Owns the Dashboard empty state. Lives here (not DashboardLayout) so that
 * the layout can always render <Outlet>, letting RouteRenderer's
 * <Navigate to="overview" replace /> index redirect fire on `/` → `/overview`
 * even when no data has been captured yet. Same empty-state-in-child pattern
 * AuditLayout adopted after the 2026-04-08 routing.spec regression.
 */

import { BarChart3, Camera, Clock, Monitor, Moon } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import StatCard from '../../components/StatCard'
import TodaySummary from '../../components/TodaySummary'
import { Badge, Card, EmptyState } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { colors, iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { formatDuration } from '../../utils/formatters'
import type { DashboardContext } from './DashboardLayout'

export default function OverviewSection() {
  const { t } = useTranslation()
  const { latestMetrics, idleState, metricsHistory, summary, isWidgetVisible } =
    useTypedOutletContext<DashboardContext>('Dashboard')

  const isEmpty =
    !latestMetrics && !summary?.events_logged && !summary?.frames_captured && (summary?.total_active_secs ?? 0) === 0

  if (isEmpty) {
    return (
      <EmptyState
        icon={<Monitor className="h-8 w-8" />}
        title={t('emptyState.dashboard.title')}
        description={t('emptyState.dashboard.description')}
      />
    )
  }

  return (
    <>
      {isWidgetVisible('overview.realtime') && latestMetrics && (
        <Card variant="default" padding="md">
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-2">
              <span className={cn('h-2 w-2 animate-pulse rounded-full', colors.primary.signal)} />
              <span className={cn(`${typography.weight.medium} text-sm`, colors.primary.text)}>
                {t('dashboard.realtimeMonitoring')}
              </span>
              {idleState?.is_idle && (
                <Badge color="warning" size="sm">
                  {t('dashboard.idle')} {Math.floor((idleState.idle_secs || 0) / 60)}
                  {t('dashboard.minutes')}
                </Badge>
              )}
            </div>
            <div className={cn('text-xs', colors.text.secondary)}>
              {new Date(latestMetrics.timestamp).toLocaleTimeString()}
            </div>
          </div>
          <div className="mt-3 grid grid-cols-2 gap-4 md:grid-cols-4">
            <div>
              <div className={cn(typography.stat.large, colors.primary.text)}>
                {latestMetrics.cpu_usage.toFixed(1)}%
              </div>
              <div className={cn('text-xs', colors.text.secondary)}>{t('dashboard.cpu')}</div>
            </div>
            <div>
              <div className={cn(typography.stat.large, colors.primary.text)}>
                {latestMetrics.memory_percent.toFixed(1)}%
              </div>
              <div className={cn('text-xs', colors.text.secondary)}>{t('dashboard.memory')}</div>
            </div>
            <div>
              <div className={cn(typography.stat.large, colors.primary.text)}>
                {(latestMetrics.memory_used / 1024 / 1024 / 1024).toFixed(1)} GB
              </div>
              <div className={cn('text-xs', colors.text.secondary)}>{t('dashboard.usedMemory')}</div>
            </div>
            <div>
              <div className={cn(typography.stat.large, colors.primary.text)}>{metricsHistory.length}</div>
              <div className={cn('text-xs', colors.text.secondary)}>{t('dashboard.collectedData')}</div>
            </div>
          </div>
        </Card>
      )}

      {isWidgetVisible('overview.today-summary') && (
        <TodaySummary
          totalActiveSecs={summary?.total_active_secs ?? 0}
          topApps={(summary?.top_apps ?? []).map((a) => ({ name: a.name }))}
        />
      )}

      {isWidgetVisible('overview.stat-cards') && (
        <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
          <StatCard
            data-testid="metric-card-active-time"
            title={t('dashboard.activeTime')}
            value={formatDuration(summary?.total_active_secs ?? 0)}
            icon={<Clock className={`${iconSize.md}`} />}
          />
          <StatCard
            data-testid="metric-card-idle-time"
            title={t('dashboard.idleTime')}
            value={formatDuration(summary?.total_idle_secs ?? 0)}
            icon={<Moon className={`${iconSize.md}`} />}
          />
          <StatCard
            data-testid="metric-card-captures"
            title={t('dashboard.captures')}
            value={summary?.frames_captured?.toLocaleString() ?? '0'}
            icon={<Camera className={`${iconSize.md}`} />}
          />
          <StatCard
            data-testid="metric-card-events"
            title={t('dashboard.events')}
            value={summary?.events_logged?.toLocaleString() ?? '0'}
            icon={<BarChart3 className={`${iconSize.md}`} />}
          />
        </div>
      )}
    </>
  )
}
