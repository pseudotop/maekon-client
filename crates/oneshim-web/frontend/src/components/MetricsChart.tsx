import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { CartesianGrid, Legend, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import type { HourlyMetrics } from '../api/client'
import { chart, chartPalette } from '../styles/tokens'
import { formatGigabytes, formatPercent } from '../utils/formatters'

interface MetricsChartProps {
  data: HourlyMetrics[]
}

function formatHour(hourStr: string): string {
  try {
    const date = new Date(hourStr)
    return date.toLocaleTimeString('ko-KR', { hour: '2-digit', minute: '2-digit' })
  } catch {
    return hourStr
  }
}

function coerceMetricValue(value: unknown): number {
  const numberValue = typeof value === 'number' ? value : Number(value)
  return Number.isFinite(numberValue) ? numberValue : 0
}

function formatMetricTooltipValue(value: unknown, name: unknown): string {
  const numberValue = coerceMetricValue(value)
  return name === 'Memory (GB)' ? formatGigabytes(numberValue) : formatPercent(numberValue)
}

export default function MetricsChart({ data }: MetricsChartProps) {
  const { t } = useTranslation()

  const chartData = useMemo(
    () =>
      (data ?? []).map((m) => ({
        hour: formatHour(m.hour),
        cpu: m.cpu_avg,
        cpuMax: m.cpu_max,
        memory: (m.memory_avg ?? 0) / (1024 * 1024 * 1024), // GB
        memoryMax: (m.memory_max ?? 0) / (1024 * 1024 * 1024),
      })),
    [data],
  )

  if (!data || data.length === 0) {
    return <div className="flex h-64 items-center justify-center text-content-muted">{t('common.noData')}</div>
  }

  return (
    <div className="h-64">
      <ResponsiveContainer width="100%" height="100%">
        <LineChart data={chartData}>
          <CartesianGrid strokeDasharray="3 3" stroke={chart.gridStroke} />
          <XAxis dataKey="hour" stroke={chart.axis.stroke} tick={chart.axis.tick} />
          <YAxis
            yAxisId="cpu"
            domain={[0, 100]}
            stroke={chart.axis.stroke}
            tick={chart.axis.tick}
            tickFormatter={(v) => `${v}%`}
          />
          <YAxis
            yAxisId="memory"
            orientation="right"
            stroke={chart.axis.stroke}
            tick={chart.axis.tick}
            tickFormatter={(v) => `${(v ?? 0).toFixed(0)}GB`}
          />
          <Tooltip
            contentStyle={chart.tooltipStyle}
            formatter={formatMetricTooltipValue}
            labelStyle={chart.labelStyle}
          />
          <Legend />
          <Line
            yAxisId="cpu"
            type="monotone"
            dataKey="cpu"
            name="CPU %"
            stroke={chartPalette[0]}
            strokeWidth={2}
            dot={false}
          />
          <Line
            yAxisId="memory"
            type="monotone"
            dataKey="memory"
            name="Memory (GB)"
            stroke={chartPalette[1]}
            strokeWidth={2}
            dot={false}
          />
        </LineChart>
      </ResponsiveContainer>
    </div>
  )
}
