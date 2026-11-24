/**
 * StatCard — one labelled number with an icon.
 *
 * #13735: the value and label never wrap or truncate. The icon sits on the
 * label's line, not beside the value, so the value gets the card's full width
 * (the zh-CN spelling of "10h 45m" is half again as wide as the English one). Callers lay
 * cards out in `statCardGrid`, so a narrow window drops a column instead of
 * splitting "6h 7m" across lines. A `detail` line may wrap; it explains a
 * missing value rather than being one.
 */
import type { ReactNode } from 'react'
import { colors, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { Card } from './ui'

/**
 * Container-width grid for StatCards: as many 13rem columns as fit. 13rem keeps
 * the longest shipped label ("Cambios de contexto") on one line beside its
 * icon, and is still narrow enough for two columns at the 800px minimum window.
 */
export const statCardGrid = 'grid grid-cols-[repeat(auto-fit,minmax(13rem,1fr))] gap-4'

export interface StatCardProps {
  title: string
  value: string
  icon: ReactNode
  /** Optional secondary line, e.g. why a value is not available yet. */
  detail?: string
  'data-testid'?: string
}

export default function StatCard({ title, value, icon, detail, 'data-testid': testId }: StatCardProps) {
  return (
    <Card variant="default" padding="md" data-testid={testId} className="min-w-0">
      <div className="flex items-center gap-2">
        <span className="shrink-0 text-brand-text">{icon}</span>
        <span
          data-testid={testId ? `${testId}-label` : undefined}
          className={cn(typography.body, colors.text.secondary, 'whitespace-nowrap')}
        >
          {title}
        </span>
      </div>
      <div
        data-testid={testId ? `${testId}-value` : undefined}
        className={cn('mt-2', typography.stat.large, colors.text.primary, 'whitespace-nowrap')}
      >
        {value}
      </div>
      {detail && <div className={cn('mt-1', typography.caption, colors.text.tertiary)}>{detail}</div>}
    </Card>
  )
}
