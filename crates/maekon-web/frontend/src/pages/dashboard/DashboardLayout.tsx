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
import { formatLocalCalendarDate, localCalendarDateFromValue } from '../../utils/localDate'
import ConnectionIndicator from './ConnectionIndicator'

function toApiDate(dateInput?: string): string {
  return localCalendarDateFromValue(dateInput) || formatLocalCalendarDate(new Date())
}

export interface DashboardContext {
  status: ConnectionStatus
  latestMetrics: MetricsUpdate | null
  idleState: IdleUpdate | null
  metricsHistory: MetricsUpdate[]
  summary: DailySummary | undefined
  summaryLoading: boolean
  /** Set when the summary request failed; `summary` may still hold an earlier result. */
  summaryError: Error | null
  refetchSummary: () => void
  /** Local calendar date (`YYYY-MM-DD`) the dashboard is showing. */
  selectedDate: string
  isToday: boolean
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

  const {
    data: summary,
    isLoading: summaryLoading,
    error: summaryError,
    refetch,
  } = useQuery({
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
    summaryError,
    refetchSummary: () => {
      void refetch()
    },
    selectedDate,
    isToday: selectedDate === formatLocalCalendarDate(new Date()),
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
          <ConnectionIndicator status={status} />
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
