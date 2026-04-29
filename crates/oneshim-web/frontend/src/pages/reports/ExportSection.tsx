/**
 * Reports export section — system metrics chart (CPU + memory).
 */

import { useTranslation } from 'react-i18next'
import { Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import { Card, CardTitle } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { chart, iconSize, palette } from '../../styles/tokens'
import { formatPercent } from '../../utils/formatters'
import { ReportsEmptyState } from './ReportsEmptyState'
import type { ReportsContext } from './ReportsLayout'

export const EXPORT_METRIC_COLORS = {
  cpuStroke: palette.amber500,
  cpuLegendStyle: { backgroundColor: palette.amber500 },
  memoryStroke: palette.teal500,
  memoryLegendStyle: { backgroundColor: palette.teal500 },
} as const

export function formatExportMetricTooltipValue(value: unknown, name: unknown): [string, string] {
  const numberValue = typeof value === 'number' ? value : Number(value)
  const safeValue = Number.isFinite(numberValue) ? numberValue : 0
  return [formatPercent(safeValue), String(name || '')]
}

export default function ExportSection() {
  const { t } = useTranslation()
  const { report, reportError } = useTypedOutletContext<ReportsContext>('Reports')

  if (reportError || !report) {
    return <ReportsEmptyState reportError={reportError} />
  }

  return (
    <Card id="section-export" padding="md">
      <CardTitle>{t('reports.systemMetrics')}</CardTitle>
      <div className="mt-4 h-48">
        <ResponsiveContainer width="100%" height="100%">
          <LineChart data={report.daily_stats}>
            <XAxis dataKey="date" tickFormatter={(d) => d.slice(5)} tick={chart.axis.tick} />
            <YAxis domain={[0, 100]} tick={chart.axis.tick} />
            <Tooltip
              contentStyle={chart.tooltipStyle}
              labelStyle={chart.labelStyle}
              formatter={formatExportMetricTooltipValue}
            />
            <Line
              type="monotone"
              dataKey="cpu_avg"
              name="CPU"
              stroke={EXPORT_METRIC_COLORS.cpuStroke}
              strokeWidth={2}
              dot={false}
            />
            <Line
              type="monotone"
              dataKey="memory_avg"
              name={t('reports.memory')}
              stroke={EXPORT_METRIC_COLORS.memoryStroke}
              strokeWidth={2}
              dot={false}
            />
          </LineChart>
        </ResponsiveContainer>
      </div>
      <div className="mt-2 flex justify-center gap-6 text-sm">
        <div className="flex items-center gap-2">
          <div className={`${iconSize.xs} rounded-full`} style={EXPORT_METRIC_COLORS.cpuLegendStyle} />
          <span className="text-content-secondary">CPU</span>
        </div>
        <div className="flex items-center gap-2">
          <div className={`${iconSize.xs} rounded-full`} style={EXPORT_METRIC_COLORS.memoryLegendStyle} />
          <span className="text-content-secondary">{t('reports.memory')}</span>
        </div>
      </div>
    </Card>
  )
}
