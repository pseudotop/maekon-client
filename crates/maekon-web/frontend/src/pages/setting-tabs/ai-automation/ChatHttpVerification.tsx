import { useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { verifyChatHttpProvider } from '../../../api/client'
import type {
  AiReadinessReasonCode,
  ChatHttpVerificationStatus,
  FeatureCapabilitySnapshot,
} from '../../../api/contracts'
import { Alert, Button } from '../../../components/ui'
import { aiCapabilityReadiness } from '../../../features/aiReadiness'
import { invalidateAiReadinessSnapshotCache } from '../../../hooks/useAiReadinessSnapshot'
import { typography } from '../../../styles/tokens'

/** `chat.http_api` blockers that one verification call can settle (#12530). */
const VERIFIABLE_REASONS: ReadonlySet<AiReadinessReasonCode> = new Set([
  'provider_invocation_unverified',
  'model_availability_unverified',
])

/** `failed` is a rejected invoke; the rest are backend statuses. */
type VerificationOutcome = ChatHttpVerificationStatus | 'failed'

const OUTCOMES: Record<VerificationOutcome, { copyKey: string; variant: 'success' | 'warning' | 'error' }> = {
  verified: { copyKey: 'settingsAutomation.chatVerification.result.verified', variant: 'success' },
  not_configured: { copyKey: 'settingsAutomation.chatVerification.result.notConfigured', variant: 'warning' },
  consent_required: { copyKey: 'settingsAutomation.chatVerification.result.consentRequired', variant: 'warning' },
  restart_required: { copyKey: 'settingsAutomation.chatVerification.result.restartRequired', variant: 'warning' },
  budget_exhausted: { copyKey: 'settingsAutomation.chatVerification.result.budgetExhausted', variant: 'warning' },
  provider_error: { copyKey: 'settingsAutomation.chatVerification.result.providerError', variant: 'error' },
  timeout: { copyKey: 'settingsAutomation.chatVerification.result.timeout', variant: 'error' },
  failed: { copyKey: 'settingsAutomation.chatVerification.result.failed', variant: 'error' },
}

interface ChatHttpVerificationProps {
  snapshot: FeatureCapabilitySnapshot | null | undefined
  /** The backend verifies the saved configuration, so unsaved edits block the check. */
  hasUnsavedChanges: boolean
}

/**
 * Settles `chat.http_api` readiness with one user-started call. The backend
 * sends a fixed synthetic message, never user or screen content, and on
 * success records evidence that readiness reads; this refreshes readiness so
 * Chat can open the path without another setup step.
 */
export default function ChatHttpVerification({ snapshot, hasUnsavedChanges }: ChatHttpVerificationProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [pending, setPending] = useState(false)
  const [outcome, setOutcome] = useState<VerificationOutcome | null>(null)

  const verifiable = VERIFIABLE_REASONS.has(aiCapabilityReadiness(snapshot, 'chat.http_api').reason_code)
  // A result stays visible after success hides the button, so it can report it.
  if (!verifiable && outcome === null) return null

  const verify = async () => {
    setPending(true)
    setOutcome(null)
    try {
      const { status } = await verifyChatHttpProvider()
      setOutcome(status)
      if (status === 'verified') {
        invalidateAiReadinessSnapshotCache()
        void queryClient.invalidateQueries({ queryKey: ['feature-capabilities'] })
      }
    } catch {
      setOutcome('failed')
    } finally {
      setPending(false)
    }
  }

  return (
    <div
      className="space-y-3 rounded-lg border border-muted bg-surface-muted/80 p-4"
      data-testid="settings-chat-http-verification"
    >
      {verifiable && (
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0 flex-1 space-y-1">
            <p className={`${typography.weight.medium} text-content-strong text-sm`}>
              {t('settingsAutomation.chatVerification.title')}
            </p>
            <p className="text-content-secondary text-sm">{t('settingsAutomation.chatVerification.description')}</p>
            {hasUnsavedChanges && (
              <p className="text-content-muted text-xs" data-testid="settings-chat-http-verification-save-first">
                {t('settingsAutomation.chatVerification.saveFirst')}
              </p>
            )}
          </div>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            isLoading={pending}
            disabled={hasUnsavedChanges}
            onClick={() => void verify()}
            data-testid="settings-chat-http-verify"
          >
            {t(
              pending ? 'settingsAutomation.chatVerification.verifying' : 'settingsAutomation.chatVerification.action',
            )}
          </Button>
        </div>
      )}
      {outcome && (
        <Alert
          variant={OUTCOMES[outcome].variant}
          data-testid="settings-chat-http-verification-result"
          data-status={outcome}
        >
          {t(OUTCOMES[outcome].copyKey)}
        </Alert>
      )}
    </div>
  )
}
