import { useTranslation } from 'react-i18next'
import type { ProcessSnapshot } from '../api/client'
import { typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { formatBytes } from '../utils/formatters'

interface ProcessListProps {
  snapshot: ProcessSnapshot
}

export default function ProcessList({ snapshot }: ProcessListProps) {
  const { t } = useTranslation()
  const processes = snapshot.processes.slice(0, 10)

  if (processes.length === 0) {
    return <div className="py-8 text-center text-content-muted">{t('common.noData', 'No data')}</div>
  }

  return (
    <div className="space-y-2">
      {processes.map((proc, index) => (
        <div key={proc.pid} className="flex items-center justify-between rounded-lg bg-surface-muted p-3">
          <div className="flex items-center space-x-3">
            <span className={cn('w-6 text-content-tertiary', typography.body)}>{index + 1}</span>
            <div>
              <div className={cn('text-content', typography.label)}>{proc.name}</div>
              <div className={cn('text-content-tertiary', typography.caption)}>PID: {proc.pid}</div>
            </div>
          </div>
          <div className="flex items-center space-x-4 text-right">
            <div>
              <div className={cn('text-brand-text', typography.label)}>{(proc.cpu_usage ?? 0).toFixed(1)}%</div>
              <div className={cn('text-content-tertiary', typography.caption)}>CPU</div>
            </div>
            <div>
              <div className={cn('text-brand-text', typography.label)}>{formatBytes(proc.memory_bytes)}</div>
              <div className={cn('text-content-tertiary', typography.caption)}>Memory</div>
            </div>
          </div>
        </div>
      ))}
    </div>
  )
}
