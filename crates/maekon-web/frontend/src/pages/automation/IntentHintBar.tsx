// #5705: Natural-language automation entry point.
// Renders a single-line input + Run button on the /automation page.
// On submit, calls executeIntentHint via the dedicated fetch path (Amendment A).
// No confirm modal — executes immediately under strict sandbox (Amendment C).
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { CheckCircle2, Clock, ExternalLink, XCircle } from 'lucide-react'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { type AutomationStatus, type ExecuteIntentHintResponse, executeIntentHint } from '../../api/client'
import { Button } from '../../components/ui/Button'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { Input } from '../../components/ui/Input'
import { iconSize, interaction, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface IntentHintBarProps {
  status: AutomationStatus | undefined
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
            <div className={cn(typography.weight.medium)}>{data.result.error ?? t('automation.intentHintFailed')}</div>
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
  error: Error
}

function ErrorPanel({ error }: ErrorPanelProps) {
  const { t } = useTranslation()
  const kind = classifyError(error.message)

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
              <span className={cn(typography.weight.medium)}>{error.message || t('automation.intentHintFailed')}</span>
              {/* #5705: remind the user the request may have executed before
                  the timeout / network error so they should check history. */}
              <div className="mt-1 text-[11px] opacity-80">{t('automation.intentHintErrorCheckHistory')}</div>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

export default function IntentHintBar({ status }: IntentHintBarProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()

  // #5705: session_id is stable per mount — not regenerated on each submit.
  const mountTs = useRef(Date.now())
  const sessionId = `intent-bar-${mountTs.current}`

  const [hint, setHint] = useState('')
  const [lastElapsedMs, setLastElapsedMs] = useState(0)

  const mutation = useMutation({
    // React Query v5 object syntax (#5705)
    mutationFn: (intentHint: string) =>
      executeIntentHint({
        command_id: `intent-hint-ui-${Date.now()}`,
        session_id: sessionId,
        intent_hint: intentHint,
      }),
    onSuccess: () => {
      // Invalidate stats so the execution count updates; audit page
      // self-refetches on its own 10s interval — no auditLogs invalidation needed.
      queryClient.invalidateQueries({ queryKey: ['automationStats'] })
    },
    onMutate: () => {
      // Record start time so we can compute elapsed_ms from mutation result
      setLastElapsedMs(0)
    },
  })

  const isDisabled = !status?.enabled
  const trimmed = hint.trim()
  const canSubmit = !isDisabled && trimmed.length > 0 && !mutation.isPending

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!canSubmit) return
    const startMs = Date.now()
    mutation.mutate(trimmed, {
      onSettled: (_data, _err) => {
        setLastElapsedMs(Date.now() - startMs)
      },
    })
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

          {/* #5705 Amendment C / #5775 FLAG: caption is conditioned on confirmation_policy.
              AUTO → existing key (immediate run); CONFIRM → requires confirmation;
              BLOCK → execution disabled. Falls back to AUTO caption when policy absent. */}
          {(status?.confirmation_policy ?? 'AUTO') === 'CONFIRM' ? (
            <p className="text-content-muted text-xs">{t('automation.intentHintRequiresConfirmation')}</p>
          ) : (status?.confirmation_policy ?? 'AUTO') === 'BLOCK' ? (
            <p className="text-content-muted text-xs">{t('automation.intentHintBlockedByPolicy')}</p>
          ) : (
            <p className="text-content-muted text-xs">{t('automation.intentHintRunsImmediately')}</p>
          )}

          {/* Disabled explanation (CommandsSection.tsx:333 parity) */}
          {isDisabled && <p className="text-content-secondary text-xs">{t('automation.intentHintDisabledHint')}</p>}
        </form>

        {/* #5705: ResultPanel handles both result.success=true and false.
            success=false comes back as a resolved mutation (not a thrown error),
            so we always show the panel when mutation.data is present. */}
        {mutation.isSuccess && mutation.data && <ResultPanel data={mutation.data} elapsedMs={lastElapsedMs} />}
        {mutation.isError && mutation.error instanceof Error && <ErrorPanel error={mutation.error} />}
      </CardContent>
    </Card>
  )
}
