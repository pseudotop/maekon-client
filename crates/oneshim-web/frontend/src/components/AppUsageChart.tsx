import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { Bar, BarChart, Cell, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import type { AppUsage } from '../api/client'
import { chart, chartPalette } from '../styles/tokens'
import { formatDuration } from '../utils/formatters'

interface AppUsageChartProps {
  apps: AppUsage[]
}

export default function AppUsageChart({ apps }: AppUsageChartProps) {
  const { t } = useTranslation()

  const chartData = useMemo(
    () =>
      (apps ?? []).slice(0, 8).map((app) => ({
        name: app.name.length > 15 ? `${app.name.slice(0, 15)}...` : app.name,
        fullName: app.name,
        duration: app.duration_secs,
        durationStr: formatDuration(app.duration_secs),
        events: app.event_count,
      })),
    [apps],
  )

  if (!apps || apps.length === 0) {
    return <div className="flex h-64 items-center justify-center text-content-muted">{t('common.noData')}</div>
  }

  return (
    <div className="h-64">
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={chartData} layout="vertical">
          <XAxis
            type="number"
            stroke={chart.axis.stroke}
            tick={chart.axis.tick}
            tickFormatter={(v) => formatDuration(v)}
          />
          <YAxis type="category" dataKey="name" stroke={chart.axis.stroke} tick={chart.axis.tick} width={100} />
          <Tooltip
            contentStyle={chart.tooltipStyle}
            labelStyle={chart.labelStyle}
            formatter={(value: number, _name: string, props: { payload?: { fullName: string } }) => [
              formatDuration(value),
              props.payload?.fullName ?? '',
            ]}
          />
          <Bar dataKey="duration" radius={[0, 4, 4, 0]}>
            {chartData.map((entry, index) => (
              <Cell key={entry.fullName} fill={chartPalette[index % chartPalette.length]} />
            ))}
          </Bar>
        </BarChart>
      </ResponsiveContainer>
    </div>
  )
}
