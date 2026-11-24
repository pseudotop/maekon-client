/**
 * System Metrics page — realtime CPU/memory, the 24h chart, the busiest
 * processes, app usage, and today's capture counts.
 *
 * #13735: this used to be the Dashboard's second tab, which made the first
 * thing people saw read like a task manager. It is now its own `/monitoring`
 * route (same URL, so deep links keep working), linked from Support. It owns
 * its SSE stream, widget settings and today's summary because it no longer
 * sits under DashboardLayout.
 */

import { useQuery } from '@tanstack/react-query'
import { BarChart3, Camera } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { fetchHourlyMetrics, fetchProcesses, fetchSummary } from '../../api/client'
import type { DailySummary } from '../../api/contracts'
import AppUsageChart from '../../components/AppUsageChart'
import MetricsChart from '../../components/MetricsChart'
import ProcessList from '../../components/ProcessList'
import SelfResourceSnapshot from '../../components/SelfResourceSnapshot'
import StatCard, { statCardGrid } from '../../components/StatCard'
import { SupportDetails } from '../../components/SupportDetails'
import { Alert, Badge, Button, Card, CardTitle } from '../../components/ui'
import WidgetCustomizer from '../../components/WidgetCustomizer'
import { useDashboardWidgets } from '../../hooks/useDashboardWidgets'
import { type IdleUpdate, type MetricsUpdate, useSSE } from '../../hooks/useSSE'
import { useErrorDisplay } from '../../i18n/errorDisplay'
import { colors, iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { formatDuration } from '../../utils/formatters'
import { formatLocalCalendarDate } from '../../utils/localDate'
import { IS_TAURI } from '../../utils/platform'
import ConnectionIndicator from './ConnectionIndicator'

/**
 * #8082 CJ-01-10: after a long session a transient fetch failure left System
 * Metrics stuck on "Error" with no recovery. Each metrics panel now surfaces
 * the failure explicitly and exposes a manual `refetch()` so the user can
 * recover without reloading the app.
 */
function MetricsErrorAlert({ title, error, onRetry }: { title: string; error: unknown; onRetry: () => void }) {
  const { t } = useTranslation()
  const describeError = useErrorDisplay()
  const display = describeError(error)
  return (
    <Alert variant="error" title={title}>
      <div className="flex items-center justify-between gap-4">
        <span>{display.text}</span>
        <Button variant="secondary" size="sm" onClick={onRetry}>
          {t('common.retry', 'Retry')}
        </Button>
      </div>
      <SupportDetails detail={display.detail} />
    </Alert>
  )
}

function RealtimeCard({
  latestMetrics,
  idleState,
  sampleCount,
}: {
  latestMetrics: MetricsUpdate
  idleState: IdleUpdate | null
  sampleCount: number
}) {
  const { t } = useTranslation()
  return (
    <Card variant="default" padding="md" data-testid="monitoring-realtime">
      <div className="flex items-center justify-between">
        <div className="flex items-center space-x-2">
          <span className={cn('h-2 w-2 animate-pulse rounded-full', colors.primary.signal)} />
          <span className={cn(`${typography.weight.medium} text-sm`, colors.primary.text)}>
            {t('dashboard.realtimeMonitoring')}
          </span>
          {idleState?.is_idle && (
            // Shared duration format, like the Dashboard's idle card; #13740 keeps
            // seconds-to-minutes math in utils/formatters.
            <Badge color="warning" size="sm" data-testid="monitoring-idle-badge">
              {t('dashboard.idle')} {formatDuration(idleState.idle_secs || 0, true)}
            </Badge>
          )}
        </div>
        <div className={cn('text-xs', colors.text.secondary)}>
          {new Date(latestMetrics.timestamp).toLocaleTimeString()}
        </div>
      </div>
      <div className="mt-3 grid grid-cols-2 gap-4 md:grid-cols-4">
        <div>
          <div className={cn(typography.stat.large, colors.primary.text)}>{latestMetrics.cpu_usage.toFixed(1)}%</div>
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
          <div className={cn(typography.stat.large, colors.primary.text)}>{sampleCount}</div>
          <div className={cn('text-xs', colors.text.secondary)}>{t('dashboard.collectedData')}</div>
        </div>
      </div>
    </Card>
  )
}

function CaptureCounts({ summary }: { summary: DailySummary | undefined }) {
  const { t } = useTranslation()
  return (
    <div className="space-y-2">
      <div className={statCardGrid}>
        <StatCard
          data-testid="metric-card-captures"
          title={t('dashboard.captures')}
          value={summary?.frames_captured?.toLocaleString() ?? '0'}
          icon={<Camera className={iconSize.md} />}
        />
        <StatCard
          data-testid="metric-card-events"
          title={t('dashboard.events')}
          value={summary?.events_logged?.toLocaleString() ?? '0'}
          icon={<BarChart3 className={iconSize.md} />}
        />
      </div>
      {/* #8077-B: make the count scope explicit — these are today's totals,
          distinct from the Storage tab's all-time totals. */}
      <p className="text-content-tertiary text-xs">
        {t(
          'dashboard.countScopeNote',
          "Captures and events reflect today's activity. See Settings > Storage for all-time totals.",
        )}
      </p>
    </div>
  )
}

export default function MonitoringSection() {
  const { t } = useTranslation()
  const { status, latestMetrics, idleState, metricsHistory } = useSSE()
  const { isVisible, canToggle, toggle, resetToDefaults } = useDashboardWidgets()
  const today = formatLocalCalendarDate(new Date())

  // Same cache entry the Dashboard uses for today.
  const { data: summary } = useQuery({
    queryKey: ['summary', today],
    queryFn: () => fetchSummary(today),
  })

  const {
    data: hourlyMetrics,
    error: hourlyMetricsError,
    isError: hourlyMetricsFailed,
    refetch: refetchHourlyMetrics,
  } = useQuery({
    queryKey: ['hourlyMetrics'],
    queryFn: () => fetchHourlyMetrics(24),
    refetchInterval: 60_000, // hourly chart — refresh every 60s
  })

  const {
    data: processes,
    error: processesError,
    isError: processesFailed,
    refetch: refetchProcesses,
  } = useQuery({
    queryKey: ['processes'],
    queryFn: () => fetchProcesses(undefined, undefined, 5),
    refetchInterval: 30_000, // process list — refresh every 30s
  })

  return (
    <div className="min-h-full space-y-6 p-6">
      <div className="flex flex-col justify-between gap-4 md:flex-row md:items-start">
        <div className="space-y-2">
          <div className="flex flex-wrap items-center gap-4">
            <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('sidebar.systemMetrics')}</h1>
            <ConnectionIndicator status={status} />
          </div>
          <p className={cn(typography.body, colors.text.secondary)}>{t('dashboard.systemResources.description')}</p>
        </div>
        <WidgetCustomizer
          section="monitoring"
          isVisible={isVisible}
          canToggle={canToggle}
          onToggle={toggle}
          onReset={resetToDefaults}
        />
      </div>

      {isVisible('monitoring.realtime') && latestMetrics && (
        <RealtimeCard latestMetrics={latestMetrics} idleState={idleState} sampleCount={metricsHistory.length} />
      )}

      {isVisible('monitoring.capture-counts') && <CaptureCounts summary={summary} />}

      {isVisible('monitoring.metrics-chart') && (
        <Card id="section-metrics" variant="default" padding="lg">
          <CardTitle className="mb-4">{t('dashboard.cpuMemory24h')}</CardTitle>
          {hourlyMetricsFailed ? (
            <MetricsErrorAlert
              title={t('dashboard.metricsError', 'Failed to load system metrics')}
              error={hourlyMetricsError}
              onRetry={() => refetchHourlyMetrics()}
            />
          ) : (
            <MetricsChart data={hourlyMetrics ?? []} />
          )}
        </Card>
      )}

      {/* #8082 / #8058: self-process resource diagnostics (desktop-only). */}
      {IS_TAURI && isVisible('monitoring.metrics-chart') && <SelfResourceSnapshot />}

      {(isVisible('monitoring.app-usage') || isVisible('monitoring.process-list')) && (
        <div id="section-processes" className="grid grid-cols-1 gap-6 lg:grid-cols-2">
          {isVisible('monitoring.app-usage') && (
            <Card variant="default" padding="lg">
              <CardTitle className="mb-4">{t('dashboard.appUsageTime')}</CardTitle>
              <AppUsageChart apps={summary?.top_apps ?? []} />
            </Card>
          )}

          {isVisible('monitoring.process-list') && (
            <Card variant="default" padding="lg">
              <CardTitle className="mb-4">{t('dashboard.recentProcesses')}</CardTitle>
              {processesFailed ? (
                <MetricsErrorAlert
                  title={t('dashboard.processesError', 'Failed to load processes')}
                  error={processesError}
                  onRetry={() => refetchProcesses()}
                />
              ) : processes && processes.length > 0 ? (
                <ProcessList snapshot={processes[0]} />
              ) : (
                <div className={cn(colors.text.secondary, 'py-8 text-center')}>{t('common.noData')}</div>
              )}
            </Card>
          )}
        </div>
      )}
    </div>
  )
}
