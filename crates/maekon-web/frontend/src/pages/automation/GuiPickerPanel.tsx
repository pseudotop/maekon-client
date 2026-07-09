// #7916 (Epic #7908 T4.2): GUI HITL picker — the first UI consumer of the
// GUI-V2 session API (client.ts:1287+). Drives the full, already-backed
// highlight → confirm → ticket → execute lifecycle over REST:
//
//   1. Scan     → createGuiSession (scene analysis + candidates) then
//                 highlightGuiSession (backend overlay driver paints the boxes
//                 in the magic-overlay window via the existing
//                 `overlay:update-focus` event — no new Tauri events needed).
//   2. Confirm  → confirmGuiSession mints an HMAC-signed execution ticket.
//   3. Execute  → executeGuiSession runs the confirmed action through the
//                 EXISTING confirmation / sandbox / policy / audit gates.
//
// This component only orchestrates the REST calls; every trust decision
// (constant-time ticket verify, nonce burn, TTL, focus-drift binding, sandbox)
// stays server-side in maekon-automation, unchanged.
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { CheckCircle2, MousePointerClick, XCircle } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import {
  type AutomationStatus,
  confirmGuiSession,
  createGuiSession,
  executeGuiSession,
  type GuiCandidate,
  type GuiCreateSessionResponse,
  type GuiExecuteResponse,
  type GuiExecutionTicket,
  highlightGuiSession,
} from '../../api/client'
import { Button } from '../../components/ui/Button'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { Input } from '../../components/ui/Input'
import { Select } from '../../components/ui/Select'
import { iconSize, interaction, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface GuiPickerPanelProps {
  status: AutomationStatus | undefined
}

type ActionType = 'click' | 'type_text'

// Human-readable label for a candidate, preferring the accessibility label,
// then the role, then the raw element id.
function candidateLabel(candidate: GuiCandidate): string {
  const el = candidate.element
  return el.label?.trim() || el.role?.trim() || el.element_id
}

interface ExecutionResultPanelProps {
  data: GuiExecuteResponse
}

function ExecutionResultPanel({ data }: ExecutionResultPanelProps) {
  const { t } = useTranslation()
  const success = data.outcome.succeeded
  return (
    <div
      className={cn(
        'mt-3 rounded-md p-3 text-sm',
        success ? 'bg-semantic-success/10 text-semantic-success' : 'bg-semantic-error/10 text-semantic-error',
      )}
      role="status"
      aria-live="polite"
      data-testid="gui-picker-result"
    >
      <div className="flex items-start gap-2">
        {success ? (
          <CheckCircle2 className={cn(iconSize.base, 'mt-0.5 shrink-0')} aria-hidden="true" />
        ) : (
          <XCircle className={cn(iconSize.base, 'mt-0.5 shrink-0')} aria-hidden="true" />
        )}
        <div className="min-w-0 flex-1">
          <div className={cn(typography.weight.medium)}>
            {success ? t('guiPicker.executionSucceeded') : t('guiPicker.executionFailed')}
          </div>
          {data.outcome.detail && <div className="mt-1 text-[11px] opacity-80">{data.outcome.detail}</div>}
          <div className="mt-2 flex items-center gap-3 text-[11px] opacity-80">
            <span>
              {t('guiPicker.resultSteps', {
                completed: data.outcome.steps_completed,
                total: data.outcome.total_steps,
              })}
            </span>
            <Link to="/automation/history" className={cn('underline-offset-2 hover:underline', interaction.focusRing)}>
              {t('guiPicker.viewHistory')}
            </Link>
          </div>
        </div>
      </div>
    </div>
  )
}

function ErrorLine({ message }: { message: string }) {
  const { t } = useTranslation()
  return (
    <div
      className="mt-3 rounded-md bg-semantic-error/10 p-3 text-semantic-error text-sm"
      role="alert"
      aria-live="assertive"
    >
      <div className="flex items-start gap-2">
        <XCircle className={cn(iconSize.base, 'mt-0.5 shrink-0')} aria-hidden="true" />
        <span className={cn(typography.weight.medium)}>{message || t('guiPicker.error')}</span>
      </div>
    </div>
  )
}

export default function GuiPickerPanel({ status }: GuiPickerPanelProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()

  const [appName, setAppName] = useState('')
  const [session, setSession] = useState<GuiCreateSessionResponse | null>(null)
  const [selectedCandidateId, setSelectedCandidateId] = useState<string | null>(null)
  const [actionType, setActionType] = useState<ActionType>('click')
  const [typeText, setTypeText] = useState('')
  const [ticket, setTicket] = useState<GuiExecutionTicket | null>(null)

  const resetFlow = () => {
    setSession(null)
    setSelectedCandidateId(null)
    setTicket(null)
    setTypeText('')
    setActionType('click')
  }

  // Step 1: scan the focused window for candidates and paint the highlights.
  const scanMutation = useMutation({
    mutationFn: async (): Promise<GuiCreateSessionResponse> => {
      const trimmed = appName.trim()
      const created = await createGuiSession(trimmed.length > 0 ? { app_name: trimmed } : {})
      const candidateIds = created.session.candidates.map((c) => c.candidate_id)
      if (candidateIds.length > 0) {
        // Drive the overlay highlight through the backend overlay driver.
        await highlightGuiSession(created.session.session_id, created.capability_token, {
          candidate_ids: candidateIds,
        })
      }
      return created
    },
    onSuccess: (created) => {
      setSession(created)
      setSelectedCandidateId(created.session.candidates[0]?.candidate_id ?? null)
      setTicket(null)
    },
  })

  // Step 2: confirm the highlighted target → mint an HMAC-signed ticket.
  const confirmMutation = useMutation({
    mutationFn: async (): Promise<GuiExecutionTicket> => {
      if (!session || !selectedCandidateId) {
        throw new Error(t('guiPicker.selectCandidate'))
      }
      const res = await confirmGuiSession(session.session.session_id, session.capability_token, {
        candidate_id: selectedCandidateId,
        action: actionType === 'type_text' ? { action_type: 'type_text', text: typeText } : { action_type: 'click' },
      })
      return res.ticket
    },
    onSuccess: (mintedTicket) => {
      setTicket(mintedTicket)
    },
  })

  // Step 3: execute the ticket through the existing sandbox/policy/audit gates.
  const executeMutation = useMutation({
    mutationFn: async (): Promise<GuiExecuteResponse> => {
      if (!session || !ticket) {
        throw new Error(t('guiPicker.error'))
      }
      return executeGuiSession(session.session.session_id, session.capability_token, { ticket })
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['automationStats'] })
    },
  })

  const isDisabled = !status?.enabled
  const busy = scanMutation.isPending || confirmMutation.isPending || executeMutation.isPending
  const candidates = session?.session.candidates ?? []
  const needsText = actionType === 'type_text'
  const canConfirm = !!session && !!selectedCandidateId && (!needsText || typeText.trim().length > 0) && !busy
  const canExecute = !!ticket && !executeMutation.isPending

  return (
    <Card id="gui-picker-panel">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <MousePointerClick className={iconSize.base} aria-hidden="true" />
          {t('guiPicker.title')}
        </CardTitle>
      </CardHeader>
      <CardContent>
        <p className="mb-3 text-content-muted text-xs">{t('guiPicker.description')}</p>

        {/* Step 1: scan */}
        <div className="flex gap-2">
          <Input
            aria-label={t('guiPicker.appNameLabel')}
            placeholder={t('guiPicker.appNamePlaceholder')}
            value={appName}
            onChange={(e) => setAppName(e.target.value)}
            disabled={isDisabled || busy}
            className="flex-1"
          />
          <Button
            type="button"
            variant="primary"
            size="md"
            isLoading={scanMutation.isPending}
            disabled={isDisabled || busy}
            onClick={() => scanMutation.mutate()}
            data-testid="gui-picker-scan"
          >
            {t('guiPicker.scanButton')}
          </Button>
        </div>

        {isDisabled && <p className="mt-2 text-content-secondary text-xs">{t('guiPicker.disabledHint')}</p>}
        {scanMutation.isError && scanMutation.error instanceof Error && (
          <ErrorLine message={scanMutation.error.message} />
        )}

        {/* Step 2: pick a highlighted candidate + action */}
        {session && (
          <div className="mt-4 space-y-3">
            <div className={cn(typography.weight.medium, 'text-sm')}>{t('guiPicker.candidatesTitle')}</div>
            {candidates.length === 0 ? (
              <p className="text-content-secondary text-xs">{t('guiPicker.noCandidates')}</p>
            ) : (
              <div className="space-y-1">
                {candidates.map((candidate) => {
                  const selected = candidate.candidate_id === selectedCandidateId
                  return (
                    <button
                      key={candidate.candidate_id}
                      type="button"
                      aria-pressed={selected}
                      disabled={busy}
                      onClick={() => {
                        setSelectedCandidateId(candidate.candidate_id)
                        setTicket(null)
                      }}
                      className={cn(
                        'flex w-full items-center justify-between rounded-md border px-3 py-2 text-left text-sm',
                        interaction.focusRing,
                        selected ? 'border-accent bg-accent/10' : 'border-muted hover:bg-surface-muted',
                      )}
                    >
                      <span className="min-w-0 flex-1 truncate">{candidateLabel(candidate)}</span>
                      {candidate.element.role && (
                        <span className="ml-2 shrink-0 text-[11px] text-content-muted">{candidate.element.role}</span>
                      )}
                    </button>
                  )
                })}
              </div>
            )}

            <div className="flex flex-wrap items-end gap-2">
              <label className="flex flex-col gap-1 text-xs">
                <span className="text-content-muted">{t('guiPicker.actionLabel')}</span>
                <Select
                  aria-label={t('guiPicker.actionLabel')}
                  value={actionType}
                  disabled={busy}
                  onChange={(e) => {
                    setActionType(e.target.value as ActionType)
                    setTicket(null)
                  }}
                  className="w-40"
                >
                  <option value="click">{t('guiPicker.actionClick')}</option>
                  <option value="type_text">{t('guiPicker.actionTypeText')}</option>
                </Select>
              </label>

              {needsText && (
                <label className="flex flex-1 flex-col gap-1 text-xs">
                  <span className="text-content-muted">{t('guiPicker.typeTextLabel')}</span>
                  <Input
                    aria-label={t('guiPicker.typeTextLabel')}
                    placeholder={t('guiPicker.typeTextPlaceholder')}
                    value={typeText}
                    disabled={busy}
                    onChange={(e) => {
                      setTypeText(e.target.value)
                      setTicket(null)
                    }}
                  />
                </label>
              )}
            </div>

            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="secondary"
                size="sm"
                isLoading={confirmMutation.isPending}
                disabled={!canConfirm}
                onClick={() => confirmMutation.mutate()}
                data-testid="gui-picker-confirm"
              >
                {t('guiPicker.confirmButton')}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={resetFlow}
                data-testid="gui-picker-reset"
              >
                {t('guiPicker.reset')}
              </Button>
            </div>

            {confirmMutation.isError && confirmMutation.error instanceof Error && (
              <ErrorLine message={confirmMutation.error.message} />
            )}

            {/* Step 3: execute the minted ticket through existing gates */}
            {ticket && (
              <div className="rounded-md border border-muted p-3">
                <div className={cn(typography.weight.medium, 'text-semantic-success text-sm')}>
                  {t('guiPicker.ticketMinted')}
                </div>
                <div className="mt-1 text-[11px] text-content-muted">
                  {t('guiPicker.ticketExpires', { expiresAt: ticket.expires_at })}
                </div>
                <Button
                  type="button"
                  variant="primary"
                  size="sm"
                  className="mt-2"
                  isLoading={executeMutation.isPending}
                  disabled={!canExecute}
                  onClick={() => executeMutation.mutate()}
                  data-testid="gui-picker-execute"
                >
                  {t('guiPicker.executeButton')}
                </Button>
              </div>
            )}

            {executeMutation.isError && executeMutation.error instanceof Error && (
              <ErrorLine message={executeMutation.error.message} />
            )}
            {executeMutation.isSuccess && executeMutation.data && <ExecutionResultPanel data={executeMutation.data} />}
          </div>
        )}
      </CardContent>
    </Card>
  )
}
