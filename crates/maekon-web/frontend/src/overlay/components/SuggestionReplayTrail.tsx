import { Check, LocateFixed, ShieldCheck, Sparkles } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { SuggestionGuiAnchorPayload, SuggestionViewDto } from '../types'

interface SuggestionReplayTrailProps {
  anchor?: SuggestionGuiAnchorPayload | null
  suggestion?: SuggestionViewDto | null
  compact?: boolean
}

export function SuggestionReplayTrail({ anchor, suggestion, compact = false }: SuggestionReplayTrailProps) {
  const { t } = useTranslation()
  if (!anchor && !suggestion) return null

  const steps = [
    {
      key: 'target',
      label: t('suggestions.replayTargetLocked', 'Target locked'),
      icon: LocateFixed,
      active: Boolean(anchor),
    },
    {
      key: 'proposal',
      label: t('suggestions.replayProposalShown', 'Proposal shown'),
      icon: Sparkles,
      active: Boolean(suggestion),
    },
    {
      key: 'consent',
      label: t('suggestions.replayConsentGate', 'Consent gate'),
      icon: ShieldCheck,
      active: true,
    },
    {
      key: 'audit',
      label: t('suggestions.replayAuditReady', 'Audit ready'),
      icon: Check,
      active: true,
    },
  ] as const

  return (
    <section
      aria-label={t('suggestions.replayTrailLabel', 'Suggestion replay trail')}
      data-testid="suggestion-replay-trail"
      className={cn(
        'border-content-inverse/5 border-b bg-content-inverse/[0.025]',
        compact ? 'px-0 py-0' : 'px-4 py-2',
      )}
    >
      <div className={cn('mb-1 text-[10px] text-content-tertiary uppercase', typography.weight.semibold)}>
        {t('suggestions.replayTrailTitle', 'Replay trail')}
      </div>
      <ol className="grid grid-cols-4 gap-1.5">
        {steps.map((step) => {
          const Icon = step.icon
          return (
            <li
              key={step.key}
              data-rum-phase={step.key}
              className={cn(
                'min-w-0 rounded-md border px-1.5 py-1',
                step.active
                  ? 'border-DEFAULT bg-surface-muted text-content-secondary'
                  : 'border-muted bg-transparent text-content-tertiary',
              )}
            >
              <div className="flex items-center gap-1">
                <Icon className={cn(iconSize.xs, 'shrink-0')} />
                <span className="truncate text-[10px]">{step.label}</span>
              </div>
            </li>
          )
        })}
      </ol>
    </section>
  )
}
