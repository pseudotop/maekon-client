/**
 * Monitoring section — CPU/Memory chart, process list, and app usage chart.
 * Owns its own queries for hourly metrics and processes.
 */

import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { fetchHourlyMetrics, fetchProcesses } from '../../api/client'
import AppUsageChart from '../../components/AppUsageChart'
import MetricsChart from '../../components/MetricsChart'
import ProcessList from '../../components/ProcessList'
import SelfResourceSnapshot from '../../components/SelfResourceSnapshot'
import { Alert, Button, Card, CardTitle } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { colors } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { IS_TAURI } from '../../utils/platform'
import type { DashboardContext } from './DashboardLayout'

/**
 * #8082 CJ-01-10: after a long session a transient fetch failure left System
 * Metrics stuck on "Error" with no recovery. Each metrics panel now surfaces
 * the failure explicitly and exposes a manual `refetch()` so the user can
 * recover without reloading the app.
 */
function MetricsErrorAlert({ title, error, onRetry }: { title: string; error: unknown; onRetry: () => void }) {
  const { t } = useTranslation()
  return (
    <Alert variant="error" title={title}>
      <div className="flex items-center justify-between gap-4">
        <span>{error instanceof Error ? error.message : String(error)}</span>
        <Button variant="secondary" size="sm" onClick={onRetry}>
          {t('common.retry', 'Retry')}
        </Button>
      </div>
    </Alert>
  )
}

export default function MonitoringSection() {
  const { t } = useTranslation()
  const { summary, isWidgetVisible } = useTypedOutletContext<DashboardContext>('Dashboard')

  const {
    data: hourlyMetrics,
    error: hourlyMetricsError,
    refetch: refetchHourlyMetrics,
  } = useQuery({
    queryKey: ['hourlyMetrics'],
    queryFn: () => fetchHourlyMetrics(24),
    refetchInterval: 60_000, // hourly chart — refresh every 60s
  })

  const {
    data: processes,
    error: processesError,
    refetch: refetchProcesses,
  } = useQuery({
    queryKey: ['processes'],
    queryFn: () => fetchProcesses(undefined, undefined, 5),
    refetchInterval: 30_000, // process list — refresh every 30s
  })

  return (
    <>
      {isWidgetVisible('monitoring.metrics-chart') && (
        <Card id="section-metrics" variant="default" padding="lg">
          <CardTitle className="mb-4">{t('dashboard.cpuMemory24h')}</CardTitle>
          {hourlyMetricsError ? (
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
      {IS_TAURI && isWidgetVisible('monitoring.metrics-chart') && <SelfResourceSnapshot />}

      {(isWidgetVisible('monitoring.app-usage') || isWidgetVisible('monitoring.process-list')) && (
        <div id="section-processes" className="grid grid-cols-1 gap-6 lg:grid-cols-2">
          {isWidgetVisible('monitoring.app-usage') && (
            <Card variant="default" padding="lg">
              <CardTitle className="mb-4">{t('dashboard.appUsageTime')}</CardTitle>
              <AppUsageChart apps={summary?.top_apps ?? []} />
            </Card>
          )}

          {isWidgetVisible('monitoring.process-list') && (
            <Card variant="default" padding="lg">
              <CardTitle className="mb-4">{t('dashboard.recentProcesses')}</CardTitle>
              {processesError ? (
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
    </>
  )
}
