/**
 * InsightCard — displays AI-generated daily narrative and highlight chips.
 */

import { colors, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { Card } from './ui'

interface DailyInsight {
  narrative: string
  highlights: Array<{
    highlight_type: string // ACHIEVEMENT, WARNING, SUGGESTION
    text: string
    segment_id?: string
  }>
}

interface InsightCardProps {
  insight: DailyInsight | null
}

const highlightConfig: Record<string, { icon: string; bg: string; text: string }> = {
  ACHIEVEMENT: { icon: '\u{1F3C6}', bg: 'bg-semantic-success/10', text: 'text-semantic-success' },
  WARNING: { icon: '\u{26A0}\u{FE0F}', bg: 'bg-semantic-warning/10', text: 'text-semantic-warning' },
  SUGGESTION: { icon: '\u{1F4A1}', bg: 'bg-brand-signal/10', text: 'text-brand-text' },
}

export default function InsightCard({ insight }: InsightCardProps) {
  if (!insight) {
    return (
      <Card variant="accent" padding="md">
        <p className={cn(typography.body, colors.text.secondary)}>No insight available for today</p>
      </Card>
    )
  }

  return (
    <Card variant="accent" padding="md">
      <p className={cn('mb-3 leading-relaxed', typography.body, colors.text.primary)}>{insight.narrative}</p>
      {insight.highlights.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {insight.highlights.map((highlight, _idx) => {
            const config = highlightConfig[highlight.highlight_type] ?? highlightConfig.SUGGESTION
            return (
              <span
                key={`${highlight.highlight_type}-${highlight.text}`}
                className={cn(
                  'inline-flex items-center gap-1.5 rounded-full px-3 py-1',
                  typography.caption,
                  typography.label,
                  config.bg,
                  config.text,
                )}
              >
                <span aria-hidden="true">{config.icon}</span>
                {highlight.text}
              </span>
            )
          })}
        </div>
      )}
    </Card>
  )
}

InsightCard.displayName = 'InsightCard'
