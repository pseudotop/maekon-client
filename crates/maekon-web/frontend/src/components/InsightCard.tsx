/**
 * InsightCard — displays AI-generated daily narrative and highlight chips.
 */

import { useTranslation } from 'react-i18next'
import type { AiSummaryArtifact } from '../api/contracts'
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
  digestProvenance?: 'heuristic'
  aiNarrative?: AiSummaryArtifact
}

const highlightConfig: Record<string, { icon: string; bg: string; text: string }> = {
  ACHIEVEMENT: { icon: '\u{1F3C6}', bg: 'bg-semantic-success/10', text: 'text-semantic-success' },
  WARNING: { icon: '\u{26A0}\u{FE0F}', bg: 'bg-semantic-warning/10', text: 'text-semantic-warning' },
  SUGGESTION: { icon: '\u{1F4A1}', bg: 'bg-brand-signal/10', text: 'text-brand-text' },
}

function providerLabelKey(provider: AiSummaryArtifact['provider_class']): string {
  return `summaryProvenance.provider.${provider ?? 'unknown'}`
}

function failureLabelKey(reason: AiSummaryArtifact['failure_reason']): string {
  return `summaryProvenance.failure.${reason ?? 'not_generated'}`
}

export default function InsightCard({ insight, digestProvenance = 'heuristic', aiNarrative = {} }: InsightCardProps) {
  const { t } = useTranslation()

  // The artifact is the provenance authority. A legacy `insight` without an
  // artifact must not be upgraded to an AI claim merely because text exists.
  if (!aiNarrative.text) {
    return (
      <Card variant="accent" padding="md">
        <div className="mb-2 flex flex-wrap gap-2">
          <span className="rounded-full bg-surface-muted px-2 py-1 text-content-secondary text-xs">
            {t(`summaryProvenance.digest.${digestProvenance}`)}
          </span>
          <span className="rounded-full bg-semantic-warning/10 px-2 py-1 text-semantic-warning text-xs">
            {t('summaryProvenance.dailyNarrativeUnavailable')}
          </span>
        </div>
        <p className={cn(typography.body, colors.text.secondary)}>{t(failureLabelKey(aiNarrative.failure_reason))}</p>
      </Card>
    )
  }

  return (
    <Card variant="accent" padding="md">
      <div className="mb-2 flex flex-wrap gap-2">
        <span className="rounded-full bg-surface-muted px-2 py-1 text-content-secondary text-xs">
          {t(`summaryProvenance.digest.${digestProvenance}`)}
        </span>
        <span className="rounded-full bg-brand-signal/10 px-2 py-1 text-brand-text text-xs">
          {t('summaryProvenance.aiDailyNarrative')} · {t(providerLabelKey(aiNarrative.provider_class))}
        </span>
        {aiNarrative.generated_at && (
          <time className="text-content-tertiary text-xs" dateTime={aiNarrative.generated_at}>
            {new Date(aiNarrative.generated_at).toLocaleString()}
          </time>
        )}
      </div>
      <p className={cn('mb-3 leading-relaxed', typography.body, colors.text.primary)}>{aiNarrative.text}</p>
      {insight && insight.highlights.length > 0 && (
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
