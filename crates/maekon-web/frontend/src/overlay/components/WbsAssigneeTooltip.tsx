import { useTranslation } from 'react-i18next'
import { interaction, motion, typography } from '../../styles/tokens'
import { useWbsAssignee } from '../hooks/useWbsAssignee'

function errorMessageKey(error: string) {
  switch (error) {
    case 'wbs_automation_disabled':
      return 'wbsAssignee.automationDisabled'
    case 'wbs_consent_required':
      return 'wbsAssignee.consentRequired'
    case 'wbs_consent_changed':
      return 'wbsAssignee.consentChanged'
    case 'wbs_policy_blocked':
      return 'wbsAssignee.policyBlocked'
    case 'wbs_sandbox_unsupported':
      return 'wbsAssignee.sandboxUnsupported'
    case 'wbs_configuration_changed':
      return 'wbsAssignee.configurationChanged'
    case 'wbs_expired':
      return 'wbsAssignee.expired'
    case 'wbs_unavailable':
      return 'wbsAssignee.requestUnavailable'
    case 'wbs_outcome_unknown':
      return 'wbsAssignee.outcomeUnknown'
    case 'wbs_readback_unavailable':
      return 'wbsAssignee.readbackUnavailable'
    default:
      return 'wbsAssignee.unavailable'
  }
}

export function WbsAssigneeTooltip({ suspended = false }: { suspended?: boolean }) {
  const { t } = useTranslation()
  const wbs = useWbsAssignee()
  const busy = ['offering', 'loading', 'executing'].includes(wbs.status)
  const disabled = suspended || busy
  const result = wbs.execution
  const verified =
    result?.disposition === 'applied_verified' &&
    result.native_outcome === 'written_unverified' &&
    result.readback === 'matches' &&
    result.audit === 'completion_accepted'
  const canStart = ['idle', 'cancelled', 'expired', 'unavailable'].includes(wbs.status)
  const actionStyle = `${interaction.focusRing} rounded-lg bg-brand px-3 py-2 text-content-inverse text-xs disabled:opacity-50`

  return (
    <section
      aria-label={t('wbsAssignee.title')}
      aria-busy={busy}
      data-testid="wbs-assignee-tooltip"
      className="absolute top-20 right-4 z-panel max-h-[calc(100vh-6rem)] w-80 max-w-[calc(100vw-2rem)] overflow-y-auto rounded-xl border border-DEFAULT bg-surface-overlay p-4 text-content shadow-md"
    >
      <h2 className={typography.h4}>{t('wbsAssignee.title')}</h2>
      {canStart && (
        <button
          data-testid="wbs-request-consent"
          type="button"
          disabled={disabled}
          onClick={() => void wbs.requestConsent()}
          className={`${actionStyle} mt-3`}
        >
          {t(wbs.status === 'idle' ? 'wbsAssignee.start' : 'wbsAssignee.restart')}
        </button>
      )}
      {wbs.offer && wbs.status === 'consent' && (
        <div data-testid="wbs-consent">
          <p className="mt-2 text-content-secondary text-xs">
            {t('wbsAssignee.consent', { document: wbs.offer.document_display_label })}
          </p>
          <button
            data-testid="wbs-consent-accept"
            type="button"
            disabled={disabled}
            onClick={() => void wbs.acceptConsent()}
            className={`${actionStyle} mt-3`}
          >
            {t('wbsAssignee.accept')}
          </button>
        </div>
      )}
      {busy && (
        <p data-testid="wbs-loading" role="status" className="mt-3 text-content-secondary text-xs">
          {t(wbs.status === 'executing' ? 'wbsAssignee.applying' : 'wbsAssignee.loading')}
        </p>
      )}
      {wbs.recommendation && wbs.status !== 'loading' && (
        <div data-testid="wbs-candidates" className="mt-3">
          {wbs.recommendation.synthetic && (
            <p data-testid="wbs-synthetic" className="mb-2 text-semantic-warning text-xs">
              {t('wbsAssignee.synthetic')}
            </p>
          )}
          <p className="text-content-secondary text-xs">
            {t('wbsAssignee.target', {
              row: wbs.recommendation.target_hint.row,
              column: wbs.recommendation.target_hint.column,
            })}
          </p>
          {wbs.recommendation.candidates.length === 0 ? (
            <p data-testid="wbs-empty" className="mt-2 text-content-secondary text-xs">
              {t('wbsAssignee.empty')}
            </p>
          ) : (
            wbs.recommendation.candidates.map((candidate) => (
              <button
                key={candidate.candidate_id}
                data-testid={`wbs-candidate-${candidate.candidate_id}`}
                type="button"
                disabled={disabled || wbs.status !== 'ready' || !candidate.eligible || !!result}
                onClick={() => void wbs.choose(candidate)}
                className={`${motion.colors} ${interaction.focusRing} mt-2 flex w-full items-center justify-between gap-2 rounded-lg border border-DEFAULT p-2 text-left text-xs disabled:opacity-50`}
              >
                <span>{candidate.display_name}</span>
                <span className="text-content-tertiary">{candidate.rank_reason}</span>
              </button>
            ))
          )}
          <button
            data-testid="wbs-refresh"
            type="button"
            disabled={disabled}
            onClick={() => void wbs.refresh()}
            className={`${interaction.focusRing} mt-3 text-brand-text text-xs disabled:opacity-50`}
          >
            {t('wbsAssignee.refresh')}
          </button>
        </div>
      )}
      {result && (
        <div role="status" aria-live="polite" className="mt-3 text-xs">
          <p data-testid="wbs-result" className={verified ? 'text-semantic-success' : 'text-semantic-warning'}>
            {t(verified ? 'wbsAssignee.appliedVerified' : 'wbsAssignee.notVerified')}
          </p>
          <dl className="mt-2 space-y-1 text-content-secondary">
            <div>
              <dt className="inline">{t('wbsAssignee.nativeLabel')}: </dt>
              <dd className="inline">{t(`wbsAssignee.native.${result.native_outcome}`)}</dd>
            </div>
            <div>
              <dt className="inline">{t('wbsAssignee.readbackLabel')}: </dt>
              <dd className="inline">{t(`wbsAssignee.readback.${result.readback}`)}</dd>
            </div>
            <div>
              <dt className="inline">{t('wbsAssignee.auditLabel')}: </dt>
              <dd className="inline">{t(`wbsAssignee.audit.${result.audit}`)}</dd>
            </div>
          </dl>
          {!verified && (
            <button
              data-testid="wbs-check-result"
              type="button"
              disabled={disabled}
              onClick={() => void wbs.checkExecution()}
              className={`${interaction.focusRing} mt-2 text-brand-text disabled:opacity-50`}
            >
              {t('wbsAssignee.checkExecution')}
            </button>
          )}
        </div>
      )}
      {wbs.error && (
        <p data-testid="wbs-unavailable" role="alert" className="mt-3 text-semantic-error text-xs">
          {t(errorMessageKey(wbs.error))}
        </p>
      )}
      {!['idle', 'cancelled', 'expired'].includes(wbs.status) && (
        <button
          data-testid="wbs-consent-cancel"
          type="button"
          disabled={suspended}
          onClick={() => void wbs.cancel()}
          className={`${interaction.focusRing} mt-3 rounded-lg border border-DEFAULT px-3 py-2 text-xs disabled:opacity-50`}
        >
          {t('wbsAssignee.cancel')}
        </button>
      )}
    </section>
  )
}
