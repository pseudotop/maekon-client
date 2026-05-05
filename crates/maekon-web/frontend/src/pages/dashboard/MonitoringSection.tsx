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
import { Card, CardTitle } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { colors } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { DashboardContext } from './DashboardLayout'

export default function MonitoringSection() {
  const { t } = useTranslation()
  const { summary, isWidgetVisible } = useTypedOutletContext<DashboardContext>('Dashboard')

  const { data: hourlyMetrics } = useQuery({
    queryKey: ['hourlyMetrics'],
    queryFn: () => fetchHourlyMetrics(24),
    refetchInterval: 60_000, // hourly chart — refresh every 60s
  })

  const { data: processes } = useQuery({
    queryKey: ['processes'],
    queryFn: () => fetchProcesses(undefined, undefined, 5),
    refetchInterval: 30_000, // process list — refresh every 30s
  })

  return (
    <>
      {isWidgetVisible('monitoring.metrics-chart') && (
        <Card id="section-metrics" variant="default" padding="lg">
          <CardTitle className="mb-4">{t('dashboard.cpuMemory24h')}</CardTitle>
          <MetricsChart data={hourlyMetrics ?? []} />
        </Card>
      )}

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
              {processes && processes.length > 0 ? (
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
