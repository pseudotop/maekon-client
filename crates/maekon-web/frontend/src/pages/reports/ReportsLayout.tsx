/**
 * Reports layout — period selector, report query, and shared report data.
 * Child routes receive data via Outlet context.
 */

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Outlet } from 'react-router-dom'
import { fetchReport, type ReportPeriod, type ReportResponse } from '../../api/client'
import { Button, ChartSkeleton, Input, Skeleton, StatCardsSkeleton } from '../../components/ui'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

export interface ReportsContext {
  /**
   * Nullable so child sections can own the empty- and error-state UX while
   * ReportsLayout always renders <Outlet>, keeping the `/reports` →
   * `/reports/activity` index redirect fire even when the report is missing.
   * Same empty-state-in-child pattern AuditLayout uses.
   */
  report: ReportResponse | null
  reportError: string | null
}

export default function ReportsLayout() {
  const { t } = useTranslation()
  const [period, setPeriod] = useState<ReportPeriod>('week')
  const [customFrom, setCustomFrom] = useState('')
  const [customTo, setCustomTo] = useState('')

  const {
    data: report,
    isLoading,
    error,
    refetch,
  } = useQuery({
    queryKey: ['report', period, customFrom, customTo],
    queryFn: () =>
      fetchReport({
        period,
        from: period === 'custom' ? customFrom : undefined,
        to: period === 'custom' ? customTo : undefined,
      }),
    enabled: period !== 'custom' || (!!customFrom && !!customTo),
  })

  const handlePeriodChange = (newPeriod: ReportPeriod) => {
    setPeriod(newPeriod)
  }

  const handleCustomSearch = () => {
    if (customFrom && customTo) {
      refetch()
    }
  }

  if (isLoading) {
    return (
      <div className="min-h-full space-y-6 p-6">
        <div className="flex items-center justify-between">
          <Skeleton className="h-8 w-40" />
          <Skeleton className="h-10 w-64" />
        </div>
        <StatCardsSkeleton count={4} />
        <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
          <ChartSkeleton />
          <ChartSkeleton />
        </div>
      </div>
    )
  }

  return (
    <div className="min-h-full space-y-6 p-6">
      {/* Header + period selector */}
      <div className="flex flex-col justify-between gap-4 md:flex-row md:items-center">
        <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('reports.title')}</h1>

        {/* Period buttons */}
        <div className="flex flex-wrap items-center gap-2">
          <Button
            data-testid="period-week"
            variant={period === 'week' ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => handlePeriodChange('week')}
          >
            {t('reports.week')}
          </Button>
          <Button
            data-testid="period-month"
            variant={period === 'month' ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => handlePeriodChange('month')}
          >
            {t('reports.month')}
          </Button>
          <Button
            data-testid="period-custom"
            variant={period === 'custom' ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => handlePeriodChange('custom')}
          >
            {t('reports.custom')}
          </Button>

          {period === 'custom' && (
            <div className="ml-2 flex items-center gap-2">
              <Input
                type="date"
                inputSize="sm"
                value={customFrom}
                onChange={(e) => setCustomFrom(e.target.value)}
                className="w-36"
              />
              <span className="text-content-tertiary">~</span>
              <Input
                type="date"
                inputSize="sm"
                value={customTo}
                onChange={(e) => setCustomTo(e.target.value)}
                className="w-36"
              />
              <Button data-testid="generate-report" variant="primary" size="sm" onClick={handleCustomSearch}>
                {t('reports.generate')}
              </Button>
            </div>
          )}
        </div>
      </div>

      <Outlet
        context={
          {
            report: report ?? null,
            reportError: error ? t('reports.error') : null,
          } satisfies ReportsContext
        }
      />
    </div>
  )
}
