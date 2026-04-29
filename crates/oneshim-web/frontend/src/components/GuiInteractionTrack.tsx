import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { fetchGuiHeatmap } from '../api/client'

interface Props {
  start?: string
  end?: string
}

export default function GuiInteractionTrack({ start, end }: Props) {
  const { t } = useTranslation()
  const { data: cells = [] } = useQuery({
    queryKey: ['gui-heatmap', start, end],
    queryFn: () => fetchGuiHeatmap(start, end),
    refetchInterval: 30_000,
  })

  if (cells.length === 0) return null

  const max = Math.max(...cells.map((c) => c.count), 1)

  return (
    <div className="mt-2">
      <span className="text-content-secondary text-xs">{t('stats.guiInteractions') ?? 'GUI Interactions'}</span>
      <div className="flex h-6 w-full gap-px rounded bg-surface-elevated">
        {cells.map((cell) => {
          const intensity = cell.count / max
          const alpha = 0.1 + intensity * 0.8
          return (
            <div
              key={cell.hour}
              className="flex-1 rounded-sm"
              style={{ backgroundColor: `rgb(var(--brand-signal) / ${alpha})` }}
              title={`${cell.hour}: ${cell.count} interactions`}
            />
          )
        })}
      </div>
    </div>
  )
}
