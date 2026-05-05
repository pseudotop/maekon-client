/**
 * Reports activity section — daily activity chart, hourly activity, and app usage.
 */

import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import {
  Bar,
  BarChart,
  Cell,
  Line,
  LineChart,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts'
import type { ReportResponse } from '../../api/client'
import { Badge, Card, CardTitle } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { chart, chartPalette, iconSize, palette, typography } from '../../styles/tokens'
import { formatDuration, formatNumber } from '../../utils/formatters'
import { ReportsEmptyState } from './ReportsEmptyState'
import type { ReportsContext } from './ReportsLayout'

const COLORS = chartPalette
const MAX_PIE_SLICES = 5

export function formatReportCountTooltipValue(value: unknown, name: unknown, locale?: string): [string, string] {
  const numberValue = typeof value === 'number' ? value : Number(value)
  const safeValue = Number.isFinite(numberValue) ? numberValue : 0
  return [formatNumber(safeValue, locale), String(name || '')]
}

function consolidateAppStats(stats: ReportResponse['app_stats']): ReportResponse['app_stats'] {
  if (stats.length <= MAX_PIE_SLICES) return stats

  const top = stats.slice(0, MAX_PIE_SLICES)
  const rest = stats.slice(MAX_PIE_SLICES)
  const otherDuration = rest.reduce((sum, s) => sum + s.duration_secs, 0)
  const otherPercentage = rest.reduce((sum, s) => sum + s.percentage, 0)
  const otherEvents = rest.reduce((sum, s) => sum + s.events, 0)
  const otherCaptures = rest.reduce((sum, s) => sum + s.captures, 0)
  return [
    ...top,
    {
      name: 'Other',
      duration_secs: otherDuration,
      percentage: otherPercentage,
      events: otherEvents,
      captures: otherCaptures,
    },
  ]
}

function AppDistributionPie({ appStats }: { appStats: ReportResponse['app_stats'] }) {
  const pieData = useMemo(() => consolidateAppStats(appStats), [appStats])

  return (
    <ResponsiveContainer width="100%" height="100%">
      <PieChart>
        <Pie
          data={pieData}
          dataKey="duration_secs"
          nameKey="name"
          cx="50%"
          cy="50%"
          outerRadius={80}
          label={({ name, percentage }) =>
            percentage >= 5 ? `${name.length > 8 ? `${name.slice(0, 8)}..` : name} ${percentage.toFixed(0)}%` : ''
          }
          labelLine={false}
          style={{ fontSize: 11 }}
        >
          {pieData.map((stat, index) => (
            <Cell key={`${stat.name}-${stat.duration_secs}`} fill={COLORS[index % COLORS.length]} />
          ))}
        </Pie>
        <Tooltip
          contentStyle={chart.tooltipStyle}
          formatter={(value: number, _name: string, props: { payload?: { percentage?: number } }) => {
            const pct = props.payload?.percentage
            return [`${formatDuration(value)}${pct != null ? ` (${pct.toFixed(1)}%)` : ''}`, '']
          }}
        />
      </PieChart>
    </ResponsiveContainer>
  )
}

export default function ActivityReport() {
  const { t, i18n } = useTranslation()
  const { report, reportError } = useTypedOutletContext<ReportsContext>('Reports')

  if (reportError || !report) {
    return <ReportsEmptyState reportError={reportError} />
  }

  return (
    <>
      {/* Daily activity chart */}
      <Card id="section-activity" padding="md">
        <CardTitle>{t('reports.dailyActivity')}</CardTitle>
        <div className="mt-4 h-64">
          <ResponsiveContainer width="100%" height="100%">
            <BarChart data={report.daily_stats}>
              <XAxis dataKey="date" tickFormatter={(d) => d.slice(5)} tick={chart.axis.tick} />
              <YAxis tick={chart.axis.tick} />
              <Tooltip
                contentStyle={chart.tooltipStyle}
                labelStyle={chart.labelStyle}
                formatter={(value, name) => formatReportCountTooltipValue(value, name, i18n.language)}
              />
              <Bar dataKey="events" name={t('reports.events')} fill={palette.teal500} />
              <Bar dataKey="captures" name={t('reports.captures')} fill={palette.blue500} />
            </BarChart>
          </ResponsiveContainer>
        </div>
      </Card>

      {/* Hourly activity chart */}
      <Card padding="md">
        <CardTitle>{t('reports.hourlyActivity')}</CardTitle>
        <div className="mt-4 h-48">
          <ResponsiveContainer width="100%" height="100%">
            <LineChart data={report.hourly_activity}>
              <XAxis dataKey="hour" tickFormatter={(h) => `${h}:00`} tick={chart.axis.tick} />
              <YAxis tick={chart.axis.tick} />
              <Tooltip
                contentStyle={chart.tooltipStyle}
                formatter={(value, name) => formatReportCountTooltipValue(value, name, i18n.language)}
                labelFormatter={(h) => `${h}:00`}
              />
              <Line
                type="monotone"
                dataKey="activity"
                name={t('reports.activityCount')}
                stroke={palette.teal500}
                strokeWidth={2}
                dot={false}
              />
            </LineChart>
          </ResponsiveContainer>
        </div>
      </Card>

      {/* App usage + distribution */}
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
        {/* App usage list */}
        <Card padding="md">
          <CardTitle>{t('reports.appUsage')}</CardTitle>
          <div className="mt-4 space-y-2">
            {report.app_stats.map((app, idx) => (
              <div key={app.name} className="flex items-center justify-between rounded bg-hover/50 p-2">
                <div className="flex items-center gap-3">
                  <div
                    className={`${iconSize.xs} rounded-full`}
                    style={{ backgroundColor: COLORS[idx % COLORS.length] }}
                  />
                  <span className={`max-w-[150px] truncate ${typography.weight.medium} text-content`}>{app.name}</span>
                </div>
                <div className="flex items-center gap-4 text-sm">
                  <span className="text-content-secondary">{formatDuration(app.duration_secs)}</span>
                  <Badge color="primary" size="sm">
                    {(app.percentage ?? 0).toFixed(1)}%
                  </Badge>
                </div>
              </div>
            ))}
            {report.app_stats.length === 0 && (
              <p className="py-4 text-center text-content-secondary">{t('common.noData')}</p>
            )}
          </div>
        </Card>

        {/* App distribution pie */}
        <Card padding="md">
          <CardTitle>{t('reports.appDistribution')}</CardTitle>
          <div className="mt-4 h-64">
            {report.app_stats.length > 0 ? (
              <AppDistributionPie appStats={report.app_stats} />
            ) : (
              <div className="flex h-full items-center justify-center text-content-secondary">{t('common.noData')}</div>
            )}
          </div>
        </Card>
      </div>
    </>
  )
}
