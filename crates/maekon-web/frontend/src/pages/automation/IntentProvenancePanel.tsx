import { useTranslation } from 'react-i18next'
import type { IntentExecutionProvenance } from '../../api/contracts'
import { typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface Props {
  provenance: IntentExecutionProvenance | null
}

/** Every displayed label describes this execution's observation, never current settings. */
export function IntentProvenancePanel({ provenance: p }: Props) {
  const { t } = useTranslation()
  const providers = {
    local_model: t('automation.intentProvenance.provider.local_model'),
    remote_model: t('automation.intentProvenance.provider.remote_model'),
    rule: t('automation.intentProvenance.provider.rule'),
    unknown: t('automation.intentProvenance.provider.unknown'),
  }
  const planning = {
    llm: t('automation.intentProvenance.planning.llm'),
    rule: t('automation.intentProvenance.planning.rule'),
    unsupported: t('automation.intentProvenance.planning.unsupported'),
    denied: t('automation.intentProvenance.planning.denied'),
    error: t('automation.intentProvenance.planning.error'),
    unknown: t('automation.intentProvenance.planning.unknown'),
  }
  const actions = {
    not_started: t('automation.intentProvenance.action.not_started'),
    denied: t('automation.intentProvenance.action.denied'),
    completed: t('automation.intentProvenance.action.completed'),
    failed: t('automation.intentProvenance.action.failed'),
    unknown: t('automation.intentProvenance.action.unknown'),
  }
  const observations = {
    complete: t('automation.intentProvenance.observation.complete'),
    partial: t('automation.intentProvenance.observation.partial'),
    legacy_unknown: t('automation.intentProvenance.observation.legacy_unknown'),
  }
  const outcomes = {
    succeeded: t('automation.intentProvenance.attemptOutcome.succeeded'),
    denied: t('automation.intentProvenance.attemptOutcome.denied'),
    timeout: t('automation.intentProvenance.attemptOutcome.timeout'),
    invalid_output: t('automation.intentProvenance.attemptOutcome.invalid_output'),
    unavailable: t('automation.intentProvenance.attemptOutcome.unavailable'),
    error: t('automation.intentProvenance.attemptOutcome.error'),
    unknown: t('automation.intentProvenance.attemptOutcome.unknown'),
  }
  const reasons = {
    denied: t('automation.intentProvenance.reason.denied'),
    timeout: t('automation.intentProvenance.reason.timeout'),
    unavailable: t('automation.intentProvenance.reason.unavailable'),
    invalid_output: t('automation.intentProvenance.reason.invalid_output'),
    provider_error: t('automation.intentProvenance.reason.provider_error'),
    confidence_rejected: t('automation.intentProvenance.reason.confidence_rejected'),
    unsupported_action: t('automation.intentProvenance.reason.unsupported_action'),
    action_failed: t('automation.intentProvenance.reason.action_failed'),
    observation_unavailable: t('automation.intentProvenance.reason.observation_unavailable'),
    legacy_unknown: t('automation.intentProvenance.reason.legacy_unknown'),
  }
  const skills = {
    used: t('automation.intentProvenance.skills.used'),
    not_used: t('automation.intentProvenance.skills.not_used'),
    unsupported: t('automation.intentProvenance.skills.unsupported'),
    unknown: t('automation.intentProvenance.skills.unknown'),
  }
  const contexts = {
    none: t('automation.intentProvenance.context.none'),
    catalog: t('automation.intentProvenance.context.catalog'),
    activated: t('automation.intentProvenance.context.activated'),
  }
  const stages = {
    started: t('automation.intentProvenance.stage.started'),
    attempt_started: t('automation.intentProvenance.stage.attempt_started'),
    attempt_finished: t('automation.intentProvenance.stage.attempt_finished'),
    planning_finished: t('automation.intentProvenance.stage.planning_finished'),
    action_started: t('automation.intentProvenance.stage.action_started'),
    action_finished: t('automation.intentProvenance.stage.action_finished'),
  }
  const booleanLabel = (value: boolean | null) =>
    value === null
      ? t('automation.intentProvenance.unknownValue')
      : value
        ? t('automation.intentProvenance.yes')
        : t('automation.intentProvenance.no')

  return (
    <section
      data-testid="intent-provenance"
      aria-label={t('automation.intentProvenance.title')}
      className="mt-3 space-y-2 rounded-md border border-border p-3 text-sm"
    >
      <h3 className={cn(typography.weight.medium, 'text-content')}>{t('automation.intentProvenance.title')}</h3>
      {!p || p.observation === 'legacy_unknown' ? (
        <p data-testid="intent-provenance-unknown" className="text-content-secondary">
          {t('automation.intentProvenance.unknown')}
        </p>
      ) : (
        <>
          <dl className="grid grid-cols-2 gap-2">
            <dt>{t('automation.intentProvenance.requested')}</dt>
            <dd>{providers[p.requested_provider_class]}</dd>
            <dt>{t('automation.intentProvenance.planned')}</dt>
            <dd data-testid="intent-observed-plan">{planning[p.planning_outcome]}</dd>
            <dt>{t('automation.intentProvenance.actionLabel')}</dt>
            <dd>
              {
                actions[
                  p.observation !== 'complete' && p.action_outcome === 'not_started' ? 'unknown' : p.action_outcome
                ]
              }
            </dd>
            <dt>{t('automation.intentProvenance.observationLabel')}</dt>
            <dd>{observations[p.observation]}</dd>
          </dl>
          {p.observation_reason && <p className="text-content-secondary">{reasons[p.observation_reason]}</p>}
          {p.configuration_reason === 'rule_only_degradation' && (
            <p data-testid="intent-configuration-degradation" className="text-content-secondary">
              {t('automation.intentProvenance.configurationOnlyRule')}
            </p>
          )}
          <ol className="space-y-2">
            {p.attempts.map((attempt, index) => (
              <li key={attempt.attempt_id} data-testid="intent-provenance-attempt" className="space-y-1">
                <p>
                  {t('automation.intentProvenance.attempt', { number: index + 1 })}:{' '}
                  {providers[attempt.effective_provider_class]} — {outcomes[attempt.outcome]}
                </p>
                {attempt.reason && <p className="text-content-secondary">{reasons[attempt.reason]}</p>}
                <p className="text-content-secondary text-xs">{skills[attempt.capabilities.skills_effective]}</p>
                <p className="text-content-secondary text-xs">
                  {t('automation.intentProvenance.skillContext', {
                    requested: booleanLabel(attempt.capabilities.skills_requested),
                    forwarded: booleanLabel(attempt.capabilities.skills_context_forwarded),
                    context: contexts[attempt.capabilities.skill_context_kind],
                  })}
                </p>
              </li>
            ))}
          </ol>
          {p.events.length > 0 && (
            <details>
              <summary className="cursor-pointer text-content-secondary">
                {t('automation.intentProvenance.steps')}
              </summary>
              <ol className="mt-2 space-y-1 text-content-secondary text-xs">
                {p.events.map((event) => (
                  <li key={event.event_id} data-testid="intent-observed-event" data-sequence={event.sequence}>
                    {event.sequence + 1}. {stages[event.kind.stage]}
                  </li>
                ))}
              </ol>
            </details>
          )}
        </>
      )}
      <p className="text-content-muted text-xs">{t('automation.intentProvenance.scope')}</p>
    </section>
  )
}
