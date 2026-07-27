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

// #8082: CPU utilization is a normalized system-wide percentage (0–100). A
// corrupt sample, or one that summed per-core busy time on a multi-core host,
// can arrive negative or above 100. We CLAMP such samples to the plausible
// [0, 100] band rather than rendering a normalized/derived figure, because the
// CPU Y-axis is already pinned to domain=[0, 100]: an unclamped 340% would
// silently draw at the ceiling while the tooltip reported a truthful-looking
// "340%", presenting an implausible value as a valid reading. Memory is a byte
// count converted to GB and is only floored at 0 (it has no fixed upper bound).
const CPU_PERCENT_MIN = 0
const CPU_PERCENT_MAX = 100

function coerceMetricValue(value: unknown): number {
  const numberValue = typeof value === 'number' ? value : Number(value)
  return Number.isFinite(numberValue) ? numberValue : 0
}

/** Clamp a CPU sample to the plausible [0, 100]% band (#8082). */
export function clampCpuPercent(value: unknown): number {
  return Math.min(CPU_PERCENT_MAX, Math.max(CPU_PERCENT_MIN, coerceMetricValue(value)))
}

/** Floor a byte/GB quantity at 0 — memory can never be negative (#8082). */
export function floorNonNegative(value: unknown): number {
  return Math.max(0, coerceMetricValue(value))
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
        cpu: clampCpuPercent(m.cpu_avg),
        cpuMax: clampCpuPercent(m.cpu_max),
        memory: floorNonNegative(m.memory_avg) / (1024 * 1024 * 1024), // GB
        memoryMax: floorNonNegative(m.memory_max) / (1024 * 1024 * 1024),
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
            name={t('metrics.cpuPercent')}
            stroke={chartPalette[0]}
            strokeWidth={2}
            dot={false}
          />
          <Line
            yAxisId="memory"
            type="monotone"
            dataKey="memory"
            name={t('metrics.memoryGb')}
            stroke={chartPalette[1]}
            strokeWidth={2}
            dot={false}
          />
        </LineChart>
      </ResponsiveContainer>
    </div>
  )
}
