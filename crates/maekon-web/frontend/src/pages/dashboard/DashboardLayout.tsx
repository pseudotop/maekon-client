/**
 * Dashboard layout — shared SSE connection, summary query, and date range state.
 * Child routes receive data via Outlet context.
 */

import { useQuery } from '@tanstack/react-query'
import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Outlet } from 'react-router-dom'
import { fetchSummary } from '../../api/client'
import type { DailySummary } from '../../api/contracts'
import { AutostartOnboardingPromptHost } from '../../components/AutostartOnboardingPromptHost'
import DateRangePicker from '../../components/DateRangePicker'
import WidgetCustomizer from '../../components/WidgetCustomizer'
import type { SectionId } from '../../components/widget-registry'
import { useDashboardWidgets } from '../../hooks/useDashboardWidgets'
import { type ConnectionStatus, type IdleUpdate, type MetricsUpdate, useSSE } from '../../hooks/useSSE'
import { useCurrentRoute } from '../../routes'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

function toApiDate(dateInput?: string): string {
  if (!dateInput) {
    return new Date().toISOString().split('T')[0]
  }

  const parsed = new Date(dateInput)
  if (Number.isNaN(parsed.getTime())) {
    return new Date().toISOString().split('T')[0]
  }

  return parsed.toISOString().split('T')[0]
}

function ConnectionIndicator({ status, t }: { status: ConnectionStatus; t: (key: string) => string }) {
  const statusConfig = {
    connecting: { color: colors.status.connecting, textKey: 'dashboard.connecting' },
    connected: { color: colors.status.connected, textKey: 'dashboard.connected' },
    disconnected: { color: colors.status.disconnected, textKey: 'dashboard.disconnected' },
    error: { color: colors.status.error, textKey: 'dashboard.error' },
  }
  const config = statusConfig[status]

  return (
    <div className={cn('flex items-center space-x-2', typography.body)}>
      <span className={cn('h-2 w-2 rounded-full', config.color, status === 'connected' && 'animate-pulse')} />
      <span className={colors.text.secondary}>{t(config.textKey)}</span>
    </div>
  )
}

export interface DashboardContext {
  status: ConnectionStatus
  latestMetrics: MetricsUpdate | null
  idleState: IdleUpdate | null
  metricsHistory: MetricsUpdate[]
  summary: DailySummary | undefined
  summaryLoading: boolean
  dateRange: { from?: string; to?: string }
  handleRangeChange: (range: { from?: string; to?: string }) => void
  isWidgetVisible: (widgetId: string) => boolean
}

export default function DashboardLayout() {
  const { t } = useTranslation()
  const [dateRange, setDateRange] = useState<{ from?: string; to?: string }>({})

  const { status, latestMetrics, idleState, metricsHistory } = useSSE()
  const { isVisible, canToggle, toggle, resetToDefaults } = useDashboardWidgets()
  const { child } = useCurrentRoute()
  const activeSection = (child?.path ?? 'overview') as SectionId

  const handleRangeChange = useCallback((from: string | undefined, to: string | undefined) => {
    setDateRange({ from, to })
  }, [])

  const selectedDate = toApiDate(dateRange.from)

  const { data: summary, isLoading: summaryLoading } = useQuery({
    queryKey: ['summary', selectedDate],
    queryFn: () => fetchSummary(selectedDate),
  })

  // Never conditionally suppress <Outlet> — same class as AuditLayout regression.
  // Sections handle undefined summary gracefully via null coalescing.
  const ctx: DashboardContext = {
    status,
    latestMetrics,
    idleState,
    metricsHistory,
    summary,
    summaryLoading,
    dateRange,
    handleRangeChange: (range) => handleRangeChange(range.from, range.to),
    isWidgetVisible: isVisible,
  }

  return (
    <div className="min-h-full space-y-6 p-6">
      {/* UI note */}
      <div id="section-overview" className="flex flex-col justify-between gap-4 md:flex-row md:items-center">
        <div className="flex items-center space-x-4">
          <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('dashboard.title')}</h1>
          <ConnectionIndicator status={status} t={t} />
        </div>
        <div className="flex items-center gap-3">
          <DateRangePicker onRangeChange={handleRangeChange} />
          <WidgetCustomizer
            section={activeSection}
            isVisible={isVisible}
            canToggle={canToggle}
            onToggle={toggle}
            onReset={resetToDefaults}
          />
        </div>
      </div>

      <AutostartOnboardingPromptHost />
      <Outlet context={ctx} />
    </div>
  )
}
