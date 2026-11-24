// #5705: Natural-language automation entry point.
// Renders a single-line input + Run button on the /automation page.
// On submit, calls executeIntentHint via the dedicated fetch path (Amendment A).
// No confirm modal; caption reflects the runtime sandbox status.
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { CheckCircle2, Clock, ExternalLink, XCircle } from 'lucide-react'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import {
  type AutomationStatus,
  type ExecuteIntentHintResponse,
  executeIntentHint,
  fetchIntentExecution,
  type IntentExecutionProvenance,
} from '../../api/client'
import { decodeIntentProvenance, intentProvenanceFromError, isOpaqueIntentId } from '../../api/intentExecution'
import { Button } from '../../components/ui/Button'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { Input } from '../../components/ui/Input'
import { iconSize, interaction, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { IntentProvenancePanel } from './IntentProvenancePanel'

interface IntentHintBarProps {
  status: AutomationStatus | undefined
}

function autoCaptionKey(status: AutomationStatus | undefined): string {
  if (!status?.sandbox_enabled) {
    return 'automation.intentHintRunsImmediatelyWithoutSandbox'
  }
  if (status.sandbox_profile === 'Strict') {
    return 'automation.intentHintRunsImmediately'
  }
  return 'automation.intentHintRunsImmediatelyWithSandbox'
}

// #5705: Classify 400 error messages into actionable guidance buckets.
// Returns 'consent' for External LLM blocked, 'setup' for planner/executor
// config-missing messages, or 'generic' for everything else.
function classifyError(message: string): 'consent' | 'setup' | 'generic' {
  if (message.includes('External LLM blocked')) return 'consent'
  if (message.includes('IntentPlanner') || message.includes('IntentExecutor')) return 'setup'
  return 'generic'
}

interface ResultPanelProps {
  data: ExecuteIntentHintResponse
  elapsedMs: number
}

function ResultPanel({ data, elapsedMs }: ResultPanelProps) {
  const { t } = useTranslation()
  const variantKey = Object.keys(data.planned_intent)[0] ?? 'Unknown'
  const success = data.result.success

  return (
    <div
      className={cn(
        'mt-3 rounded-md p-3 text-sm',
        success ? 'bg-semantic-success/10 text-semantic-success' : 'bg-semantic-error/10 text-semantic-error',
      )}
      role="status"
      aria-live="polite"
    >
      <div className="flex items-start gap-2">
        {success ? (
          <CheckCircle2 className={cn(iconSize.base, 'mt-0.5 shrink-0')} aria-hidden="true" />
        ) : (
          <XCircle className={cn(iconSize.base, 'mt-0.5 shrink-0')} aria-hidden="true" />
        )}
        <div className="min-w-0 flex-1">
          {success ? (
            <>
              {/* #5705: Show the top-level intent variant name from the enum key */}
              <div className={cn(typography.weight.medium)}>
                {t('automation.intentHintPlannedIntent')}: <span>{variantKey}</span>
              </div>
              {/* Compact JSON detail of the variant payload */}
              <pre className="mt-1 overflow-x-auto whitespace-pre-wrap break-all text-[11px] opacity-80">
                {JSON.stringify(data.planned_intent[variantKey], null, 2)}
              </pre>
            </>
          ) : (
            <div className={cn(typography.weight.medium)}>{t('automation.intentHintFailed')}</div>
          )}

          <div className="mt-2 flex items-center gap-3 text-[11px] opacity-80">
            {elapsedMs > 0 && (
              <span className="flex items-center gap-1">
                <Clock className={iconSize.xs} aria-hidden="true" />
                {elapsedMs}ms
              </span>
            )}
            {/* #5705 Amendment D: history link renders in both live and demo
                mode, but do not assert audit rows light up in demo tests —
                standalone audit returns []. */}
            <Link
              to="/automation/history"
              className={cn('flex items-center gap-1 underline-offset-2 hover:underline', interaction.focusRing)}
            >
              <ExternalLink className={iconSize.xs} aria-hidden="true" />
              {t('automation.intentHintViewHistory')}
            </Link>
          </div>
        </div>
      </div>
    </div>
  )
}

interface ErrorPanelProps {
  error: unknown
  uncertain: boolean
}

function ErrorPanel({ error, uncertain }: ErrorPanelProps) {
  const { t } = useTranslation()
  // raw-error-text: matched against backend markers to pick guidance, never rendered.
  // #13737: unlike other screens this panel offers no support details — an executor
  // error can carry window titles and prompt text (IntentHintRecovery.test.tsx).
  const kind = error instanceof Error ? classifyError(error.message) : 'generic'

  return (
    <div
      className="mt-3 rounded-md bg-semantic-error/10 p-3 text-semantic-error text-sm"
      role="alert"
      aria-live="assertive"
    >
      <div className="flex items-start gap-2">
        <XCircle className={cn(iconSize.base, 'mt-0.5 shrink-0')} aria-hidden="true" />
        <div className="min-w-0 flex-1">
          {/* #5705 Amendment B: consent-fail-closed and config-missing 400s
              get actionable guidance instead of the raw error string. */}
          {kind === 'consent' && (
            <div>
              <span className={cn(typography.weight.medium)}>{t('automation.intentHintBlockedConsent')}</span>
              {' — '}
              <Link to="/settings" className={cn('underline-offset-2 hover:underline', interaction.focusRing)}>
                {t('settings.title')}
              </Link>
            </div>
          )}
          {kind === 'setup' && (
            <div>
              <span className={cn(typography.weight.medium)}>{t('automation.intentHintSetupLlm')}</span>
              {' — '}
              <Link
                to="/settings/ai-automation"
                className={cn('underline-offset-2 hover:underline', interaction.focusRing)}
              >
                {t('settings.title')}
              </Link>
            </div>
          )}
          {kind === 'generic' && (
            <div>
              <span className={cn(typography.weight.medium)}>{t('automation.intentProvenance.resultUnavailable')}</span>
              {/* #5705: remind the user the request may have executed before
                  the timeout / network error so they should check history. */}
              {uncertain && (
                <div className="mt-1 text-[11px] opacity-80">{t('automation.intentHintErrorCheckHistory')}</div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

interface SubmissionSnapshot {
  id: string
  hint: string
  provenance: IntentExecutionProvenance | null
}

export default function IntentHintBar({ status }: IntentHintBarProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const mountTs = useRef(Date.now())
  const sessionId = `intent-bar-${mountTs.current}`
  const pendingRef = useRef(false)
  const currentRef = useRef<SubmissionSnapshot | null>(null)
  const recoveryRef = useRef<string | null>(null)
  const [submission, setSubmission] = useState<SubmissionSnapshot | null>(null)
  const [recovery, setRecovery] = useState<'idle' | 'pending' | 'found' | 'unknown'>('idle')
  const [identityUnavailable, setIdentityUnavailable] = useState(false)
  const [hint, setHint] = useState('')
  const [lastElapsedMs, setLastElapsedMs] = useState(0)

  const recordObservation = (id: string, provenance: IntentExecutionProvenance | null): boolean => {
    const current = currentRef.current
    if (!current || current.id !== id) return false
    if (current.provenance && provenance && current.provenance.execution_id !== provenance.execution_id) return false
    const next = { ...current, provenance }
    currentRef.current = next
    setSubmission(next)
    return true
  }

  const mutation = useMutation({
    retry: false,
    networkMode: 'always',
    mutationFn: (input: { id: string; hint: string; retryOf?: string; startedAt: number }) =>
      executeIntentHint({
        command_id: `intent-hint-ui-${input.id}`,
        session_id: sessionId,
        intent_hint: input.hint,
        submission_id: input.id,
        ...(input.retryOf ? { retry_of: input.retryOf } : {}),
      }),
    onSuccess: (data, input) => {
      recordObservation(input.id, decodeIntentProvenance(data.execution_provenance, input.id))
      queryClient.invalidateQueries({ queryKey: ['automationStats'] })
    },
    onError: (error, input) => {
      recordObservation(input.id, intentProvenanceFromError(error, input.id))
    },
    onSettled: (_data, _error, input) => {
      if (currentRef.current?.id !== input.id) return
      pendingRef.current = false
      setLastElapsedMs(Date.now() - input.startedAt)
    },
  })

  const isDisabled = !status?.enabled
  const trimmed = hint.trim()
  const canSubmit = !isDisabled && trimmed.length > 0 && !mutation.isPending

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!canSubmit || pendingRef.current) return
    let id: string
    try {
      id = globalThis.crypto.randomUUID()
      if (!isOpaqueIntentId(id)) throw new Error('Invalid correlation')
    } catch {
      setIdentityUnavailable(true)
      return
    }
    const prior = currentRef.current
    const knownPrior =
      prior?.hint === trimmed &&
      prior.provenance?.observation === 'complete' &&
      prior.provenance.action_outcome !== 'unknown'
        ? prior.provenance.execution_id
        : undefined
    const next = { id, hint: trimmed, provenance: null }
    // #12061: Identity and an atomic guard exist before any mutation dispatch.
    pendingRef.current = true
    currentRef.current = next
    setSubmission(next)
    setRecovery('idle')
    setIdentityUnavailable(false)
    setLastElapsedMs(0)
    mutation.mutate({ id, hint: trimmed, retryOf: knownPrior, startedAt: Date.now() })
  }

  const observeSubmission = async () => {
    const id = currentRef.current?.id
    if (!id || pendingRef.current || recoveryRef.current === id) return
    recoveryRef.current = id
    setRecovery('pending')
    try {
      const result = decodeIntentProvenance(await fetchIntentExecution(id), id)
      if (currentRef.current?.id !== id) return
      setRecovery(result && recordObservation(id, result) ? 'found' : 'unknown')
    } catch {
      if (currentRef.current?.id === id) setRecovery('unknown')
    } finally {
      if (recoveryRef.current === id) recoveryRef.current = null
    }
  }

  return (
    <Card id="intent-hint-bar">
      <CardHeader>
        <CardTitle>{t('automation.intentHintTitle')}</CardTitle>
      </CardHeader>
      <CardContent>
        <form onSubmit={handleSubmit} className="space-y-2">
          <div className="flex gap-2">
            <Input
              data-testid="intent-hint-input"
              aria-label={t('automation.intentHintTitle')}
              placeholder={t('automation.intentHintPlaceholder')}
              value={hint}
              onChange={(e) => setHint(e.target.value)}
              disabled={isDisabled || mutation.isPending}
              className="flex-1"
            />
            <Button
              type="submit"
              variant="primary"
              size="md"
              isLoading={mutation.isPending}
              disabled={!canSubmit}
              data-testid="intent-hint-run"
            >
              {t('automation.intentHintRun')}
            </Button>
          </div>

          {/* #5705 Amendment C / #5775 / #6751: branch by policy first, then by
              actual sandbox status so AUTO copy does not overstate containment. */}
          {(status?.confirmation_policy ?? 'AUTO') === 'CONFIRM' ? (
            <p className="text-content-muted text-xs">{t('automation.intentHintRequiresConfirmation')}</p>
          ) : (status?.confirmation_policy ?? 'AUTO') === 'BLOCK' ? (
            <p className="text-content-muted text-xs">{t('automation.intentHintBlockedByPolicy')}</p>
          ) : (
            <p className="text-content-muted text-xs">{t(autoCaptionKey(status))}</p>
          )}

          {/* Disabled explanation (CommandsSection.tsx:333 parity) */}
          {isDisabled && <p className="text-content-secondary text-xs">{t('automation.intentHintDisabledHint')}</p>}
        </form>

        {/* #5705: ResultPanel handles both result.success=true and false.
            success=false comes back as a resolved mutation (not a thrown error),
            so we always show the panel when mutation.data is present. */}
        {mutation.isSuccess && mutation.data && <ResultPanel data={mutation.data} elapsedMs={lastElapsedMs} />}
        {identityUnavailable && <p role="alert">{t('automation.intentProvenance.identityUnavailable')}</p>}
        {mutation.isError && recovery !== 'found' && (
          <ErrorPanel error={mutation.error} uncertain={submission?.provenance?.observation !== 'complete'} />
        )}
        {submission && !mutation.isPending && (
          <>
            <IntentProvenancePanel provenance={submission.provenance} />
            <div className="mt-3 flex flex-wrap items-center gap-3">
              <Button
                type="button"
                variant="secondary"
                size="sm"
                onClick={observeSubmission}
                isLoading={recovery === 'pending'}
                disabled={recovery === 'pending'}
                data-testid="intent-observe-current"
              >
                {t('automation.intentProvenance.observe')}
              </Button>
              {!mutation.isSuccess && (
                <Link to="/automation/history" className={cn('text-sm underline', interaction.focusRing)}>
                  {t('automation.intentHintViewHistory')}
                </Link>
              )}
            </div>
            {recovery === 'unknown' && (
              <p role="status" className="mt-2 text-content-secondary text-xs">
                {t('automation.intentProvenance.lookupUnknown')}
              </p>
            )}
            {recovery === 'found' && (
              <p role="status" className="mt-2 text-content-secondary text-xs">
                {t('automation.intentProvenance.lookupFound')}
              </p>
            )}
            {submission.provenance?.observation !== 'complete' && (
              <p className="mt-2 text-content-secondary text-xs">{t('automation.intentProvenance.replayWarning')}</p>
            )}
          </>
        )}
      </CardContent>
    </Card>
  )
}
