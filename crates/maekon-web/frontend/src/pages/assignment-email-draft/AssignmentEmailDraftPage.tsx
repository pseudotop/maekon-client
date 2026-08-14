import { AlertTriangle, ExternalLink, Mail, Save, ShieldCheck } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useSearchParams } from 'react-router-dom'
import {
  type AssignmentEmailDraft,
  generateAssignmentEmailDraft,
  loadAssignmentEmailDraft,
  regenerateAssignmentEmailDraft,
} from '../../api/assignmentEmailDraft'
import {
  HANDOFF_ERROR_CODES,
  HandoffBridgeUnavailableError,
  isHandoffError,
  openExternalTarget,
} from '../../api/osHandoff'
import { Alert, Button, Card, CardContent, CardHeader, CardTitle, Checkbox, Input } from '../../components/ui'
import { iconSize, interaction, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { clearLocalDraft, loadLocalDraft, saveLocalDraft } from './draftSession'

const MAX_SUBJECT_CHARS = 180
const MAX_BODY_CHARS = 1_200
const MAX_HANDOFF_URL_CHARS = 8_000

type PageError = 'load' | 'save' | 'validation' | 'noHandler' | 'handoff' | 'bridge'

export function buildComposeTarget(recipient: string, subject: string, body: string): string {
  return `mailto:${recipient}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(body)}`
}

function validateEditor(subject: string, body: string, recipient: string): string | null {
  if (!subject.trim() || subject.length > MAX_SUBJECT_CHARS || /[\r\n]/u.test(subject)) return 'subject'
  if (!body.trim() || body.length > MAX_BODY_CHARS) return 'body'
  if (/[\r\n]/u.test(recipient)) return 'recipient'
  if (buildComposeTarget(recipient, subject, body).length > MAX_HANDOFF_URL_CHARS) return 'target'
  return null
}

export default function AssignmentEmailDraftPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const [receiptId, setReceiptId] = useState(searchParams.get('receipt') ?? '')
  const [draft, setDraft] = useState<AssignmentEmailDraft | null>(null)
  const [subject, setSubject] = useState('')
  const [body, setBody] = useState('')
  const [savedSignature, setSavedSignature] = useState('')
  const [confirmed, setConfirmed] = useState(false)
  const [busy, setBusy] = useState(false)
  const [handoffBusy, setHandoffBusy] = useState(false)
  const [handedOff, setHandedOff] = useState(false)
  const [error, setError] = useState<PageError | null>(null)
  const errorRef = useRef<HTMLDivElement>(null)
  const handoffInFlightRef = useRef(false)
  const handoffCompletedRef = useRef(false)
  const draftId = searchParams.get('draft')

  const currentSignature = `${subject}\u0000${body}`
  const dirty = draft !== null && currentSignature !== savedSignature
  const validation = draft ? validateEditor(subject, body, draft.recipient.address) : null
  const composeTarget = useMemo(
    () => (draft ? buildComposeTarget(draft.recipient.address, subject, body) : ''),
    [draft, subject, body],
  )

  const applyLoadedDraft = useCallback((loaded: AssignmentEmailDraft) => {
    const local = loaded.stale ? null : loadLocalDraft(loaded)
    if (loaded.stale) clearLocalDraft(loaded.draft_id)
    const nextSubject = local?.subject ?? loaded.subject
    const nextBody = local?.body ?? loaded.body
    setDraft(loaded)
    setReceiptId(loaded.assignment_receipt_id)
    setSubject(nextSubject)
    setBody(nextBody)
    setSavedSignature(`${nextSubject}\u0000${nextBody}`)
    setConfirmed(false)
    setHandedOff(false)
    handoffInFlightRef.current = false
    handoffCompletedRef.current = false
    setError(null)
  }, [])

  useEffect(() => {
    if (!draftId) return
    let active = true
    setBusy(true)
    loadAssignmentEmailDraft(draftId)
      .then((loaded) => {
        if (active) applyLoadedDraft(loaded)
      })
      .catch(() => active && setError('load'))
      .finally(() => active && setBusy(false))
    return () => {
      active = false
    }
  }, [draftId, applyLoadedDraft])

  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  async function generate() {
    if (!receiptId.trim()) {
      setError('validation')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const generated = await generateAssignmentEmailDraft(receiptId.trim())
      applyLoadedDraft(generated)
      navigate(`/assignment-email-draft?draft=${encodeURIComponent(generated.draft_id)}`, { replace: true })
    } catch {
      setError('load')
    } finally {
      setBusy(false)
    }
  }

  async function regenerate() {
    if (!draft || !receiptId.trim()) return
    setBusy(true)
    setError(null)
    try {
      const regenerated = await regenerateAssignmentEmailDraft(draft.draft_id, receiptId.trim())
      clearLocalDraft(draft.draft_id)
      applyLoadedDraft(regenerated)
      navigate(`/assignment-email-draft?draft=${encodeURIComponent(regenerated.draft_id)}`, { replace: true })
    } catch {
      setError('load')
    } finally {
      setBusy(false)
    }
  }

  function save() {
    if (!draft || validation) {
      setError(validation ? 'validation' : 'save')
      return
    }
    try {
      saveLocalDraft(draft, subject, body)
      setSavedSignature(currentSignature)
      setConfirmed(false)
      setHandedOff(false)
      handoffCompletedRef.current = false
      setError(null)
    } catch {
      setError('save')
    }
  }

  function cancel() {
    if (draft) clearLocalDraft(draft.draft_id)
    navigate('/home')
  }

  async function handoff() {
    if (
      !draft ||
      draft.stale ||
      dirty ||
      validation ||
      !confirmed ||
      handoffBusy ||
      handedOff ||
      handoffInFlightRef.current ||
      handoffCompletedRef.current
    ) {
      setError('validation')
      return
    }
    handoffInFlightRef.current = true
    setHandoffBusy(true)
    setError(null)
    try {
      await openExternalTarget(composeTarget)
      handoffCompletedRef.current = true
      setHandedOff(true)
      setConfirmed(false)
    } catch (caught) {
      if (isHandoffError(caught, HANDOFF_ERROR_CODES.noHandler)) setError('noHandler')
      else if (caught instanceof HandoffBridgeUnavailableError) setError('bridge')
      else setError('handoff')
    } finally {
      handoffInFlightRef.current = false
      setHandoffBusy(false)
    }
  }

  function editSubject(value: string) {
    setSubject(value)
    setConfirmed(false)
    setHandedOff(false)
    handoffCompletedRef.current = false
  }

  function editBody(value: string) {
    setBody(value)
    setConfirmed(false)
    setHandedOff(false)
    handoffCompletedRef.current = false
  }

  return (
    <main className="mx-auto w-full max-w-4xl space-y-4 p-4 sm:p-6" data-testid="assignment-email-draft">
      <header>
        <h1 className={cn(typography.h2, 'text-content')}>{t('assignmentEmailDraft.title')}</h1>
        <p className={cn(typography.body, 'mt-1 text-content-secondary')}>{t('assignmentEmailDraft.description')}</p>
      </header>

      <Alert variant="info" icon={<ShieldCheck aria-hidden="true" />} data-testid="assignment-email-provider-zero">
        {t('assignmentEmailDraft.providerZero')}
      </Alert>

      {error && (
        <Alert ref={errorRef} tabIndex={-1} variant="error" data-testid={`assignment-email-error-${error}`}>
          {t(`assignmentEmailDraft.errors.${error}`)}
        </Alert>
      )}

      {!draft && (
        <Card>
          <CardHeader>
            <CardTitle>{t('assignmentEmailDraft.source.title')}</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <label htmlFor="assignment-receipt-id" className={cn(typography.label, 'block text-content')}>
              {t('assignmentEmailDraft.source.receipt')}
            </label>
            <Input
              id="assignment-receipt-id"
              value={receiptId}
              onChange={(event) => setReceiptId(event.target.value)}
              disabled={busy}
              data-testid="assignment-email-receipt"
            />
            <Button onClick={generate} isLoading={busy} data-testid="assignment-email-generate">
              {t('assignmentEmailDraft.actions.generate')}
            </Button>
          </CardContent>
        </Card>
      )}

      {draft && (
        <>
          {draft.stale && (
            <Alert variant="warning" icon={<AlertTriangle aria-hidden="true" />} data-testid="assignment-email-stale">
              <p>{t('assignmentEmailDraft.stale.body')}</p>
              <label htmlFor="latest-assignment-receipt" className={cn(typography.label, 'mt-3 block text-content')}>
                {t('assignmentEmailDraft.stale.latestReceipt')}
              </label>
              <Input
                id="latest-assignment-receipt"
                className="mt-1"
                value={receiptId}
                onChange={(event) => setReceiptId(event.target.value)}
                disabled={busy}
                data-testid="assignment-email-latest-receipt"
              />
              <div className="mt-3 flex flex-wrap gap-2">
                <Button onClick={regenerate} isLoading={busy} data-testid="assignment-email-regenerate">
                  {t('assignmentEmailDraft.actions.regenerate')}
                </Button>
                <Button variant="secondary" onClick={cancel} data-testid="assignment-email-cancel-stale">
                  {t('common.cancel')}
                </Button>
              </div>
            </Alert>
          )}

          <Card data-testid="assignment-email-editor">
            <CardHeader>
              <CardTitle>{t('assignmentEmailDraft.editor.title')}</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <dl className="grid gap-3 rounded-md border border-muted bg-surface-subtle p-3 sm:grid-cols-2">
                <div>
                  <dt className={cn(typography.caption, 'text-content-secondary')}>
                    {t('assignmentEmailDraft.fields.organization')}
                  </dt>
                  <dd className={cn(typography.body, 'text-content')} data-testid="assignment-email-counterparty">
                    {draft.recipient.organization_label}
                  </dd>
                </div>
                <div>
                  <dt className={cn(typography.caption, 'text-content-secondary')}>
                    {t('assignmentEmailDraft.fields.contact')}
                  </dt>
                  <dd className={cn(typography.body, 'text-content')} data-testid="assignment-email-external-contact">
                    {draft.recipient.contact_label} · {t('assignmentEmailDraft.externalBadge')}
                  </dd>
                </div>
                <div className="sm:col-span-2">
                  <dt className={cn(typography.caption, 'text-content-secondary')}>
                    {t('assignmentEmailDraft.fields.recipient')}
                  </dt>
                  <dd
                    className={cn(typography.body, 'break-all text-content')}
                    data-testid="assignment-email-recipient"
                  >
                    {draft.recipient.address}
                  </dd>
                </div>
              </dl>

              <div>
                <label htmlFor="assignment-email-subject" className={cn(typography.label, 'mb-1 block text-content')}>
                  {t('assignmentEmailDraft.fields.subject')}
                </label>
                <Input
                  id="assignment-email-subject"
                  value={subject}
                  onChange={(event) => editSubject(event.target.value)}
                  maxLength={MAX_SUBJECT_CHARS}
                  disabled={draft.stale}
                  error={validation === 'subject'}
                  data-testid="assignment-email-subject"
                />
              </div>
              <div>
                <label htmlFor="assignment-email-body" className={cn(typography.label, 'mb-1 block text-content')}>
                  {t('assignmentEmailDraft.fields.body')}
                </label>
                <textarea
                  id="assignment-email-body"
                  className={cn(
                    'min-h-64 w-full resize-y rounded-md border border-muted bg-surface px-3 py-2 text-content',
                    interaction.focusRing,
                  )}
                  value={body}
                  onChange={(event) => editBody(event.target.value)}
                  maxLength={MAX_BODY_CHARS}
                  disabled={draft.stale}
                  aria-invalid={validation === 'body' || validation === 'target' || undefined}
                  data-testid="assignment-email-body"
                />
              </div>

              <div className="flex flex-wrap items-center gap-2">
                <Button
                  onClick={save}
                  disabled={draft.stale || !dirty || Boolean(validation)}
                  data-testid="assignment-email-save"
                >
                  <Save className={cn(iconSize.base, 'mr-1.5')} aria-hidden="true" />
                  {t('common.save')}
                </Button>
                <Button variant="secondary" onClick={cancel} data-testid="assignment-email-cancel">
                  {t('common.cancel')}
                </Button>
                <span
                  className={cn(typography.caption, 'text-content-secondary')}
                  aria-live="polite"
                  data-testid="assignment-email-save-state"
                >
                  {dirty ? t('assignmentEmailDraft.editor.unsaved') : t('assignmentEmailDraft.editor.saved')}
                </span>
              </div>
            </CardContent>
          </Card>

          <Card data-testid="assignment-email-handoff-review">
            <CardHeader>
              <CardTitle>{t('assignmentEmailDraft.review.title')}</CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              <p className={cn(typography.body, 'text-content-secondary')}>{t('assignmentEmailDraft.review.body')}</p>
              <Checkbox
                checked={confirmed}
                onChange={(event) => setConfirmed(event.target.checked)}
                disabled={draft.stale || dirty || Boolean(validation) || handedOff}
                label={t('assignmentEmailDraft.review.confirm')}
                description={t('assignmentEmailDraft.review.noAttachment')}
                data-testid="assignment-email-confirm"
              />
              <Button
                onClick={handoff}
                isLoading={handoffBusy}
                disabled={draft.stale || dirty || Boolean(validation) || !confirmed || handedOff}
                data-testid="assignment-email-handoff"
              >
                <Mail className={cn(iconSize.base, 'mr-1.5')} aria-hidden="true" />
                {t('assignmentEmailDraft.actions.reviewInMail')}
                <ExternalLink className="ml-1.5 h-3.5 w-3.5" aria-hidden="true" />
              </Button>
              {handedOff && (
                <Alert variant="success" data-testid="assignment-email-handed-off">
                  {t('assignmentEmailDraft.review.opened')}
                </Alert>
              )}
            </CardContent>
          </Card>
        </>
      )}
    </main>
  )
}
