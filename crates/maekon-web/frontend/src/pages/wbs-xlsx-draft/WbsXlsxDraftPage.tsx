import { AlertTriangle, CheckCircle2, FileSpreadsheet, FolderOpen } from 'lucide-react'
import { type RefObject, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useSearchParams } from 'react-router-dom'
import { type GenerateWbsXlsxResult, generateWbsXlsxDraft } from '../../api/wbsXlsxDraft'
import { Alert, Badge, Button, Card, CardContent, CardHeader, CardTitle, Input } from '../../components/ui'
import { iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

type PageState = 'idle' | 'running' | 'error'

export default function WbsXlsxDraftPage() {
  const { t } = useTranslation()
  const [searchParams] = useSearchParams()
  const [receiptId, setReceiptId] = useState(searchParams.get('receipt') ?? '')
  const [state, setState] = useState<PageState>('idle')
  const [result, setResult] = useState<GenerateWbsXlsxResult | null>(null)
  const statusRef = useRef<HTMLDivElement>(null)
  const generationIdRef = useRef(0)
  const invocationActiveRef = useRef(false)
  const linkedReceiptId = searchParams.get('receipt') ?? ''

  useEffect(() => {
    generationIdRef.current += 1
    setReceiptId(linkedReceiptId)
    setResult(null)
    setState(invocationActiveRef.current ? 'running' : 'idle')
  }, [linkedReceiptId])

  useEffect(() => {
    if (state === 'error' || result) statusRef.current?.focus()
  }, [result, state])

  function generate() {
    const normalized = receiptId.trim()
    if (!normalized) {
      setState('error')
      setResult(null)
      return
    }
    if (invocationActiveRef.current) return
    const generationId = generationIdRef.current + 1
    generationIdRef.current = generationId
    invocationActiveRef.current = true
    setState('running')
    setResult(null)
    void generateWbsXlsxDraft(normalized)
      .then((generated) => {
        invocationActiveRef.current = false
        if (generationIdRef.current !== generationId) {
          setState('idle')
          return
        }
        if (generated.outcome === 'error') {
          setResult(generated)
          setState('idle')
          return
        }
        setResult(generated)
        setState('idle')
      })
      .catch(() => {
        invocationActiveRef.current = false
        if (generationIdRef.current !== generationId) {
          setState('idle')
          return
        }
        setResult({ outcome: 'error', error_kind: 'api', code: 'service.unavailable' })
        setState('idle')
      })
  }

  return (
    <main className="mx-auto w-full max-w-4xl space-y-4 p-4 sm:p-6" data-testid="wbs-xlsx-draft">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className={cn(typography.h2, 'text-content')}>{t('wbsXlsxDraft.title')}</h1>
          <p className={cn(typography.body, 'mt-1 text-content-secondary')}>{t('wbsXlsxDraft.description')}</p>
        </div>
        <Badge
          color="warning"
          role="status"
          aria-label={t('wbsXlsxDraft.syntheticAria')}
          data-testid="wbs-xlsx-synthetic-badge"
        >
          {t('wbsXlsxDraft.synthetic')}
        </Badge>
      </header>

      <Alert
        variant="info"
        title={t('wbsXlsxDraft.nativeBoundary.title')}
        icon={<FolderOpen className={iconSize.md} />}
      >
        {t('wbsXlsxDraft.nativeBoundary.body')}
      </Alert>

      <Card>
        <CardHeader>
          <CardTitle>{t('wbsXlsxDraft.source.title')}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <label className="block space-y-1">
            <span className={typography.label}>{t('wbsXlsxDraft.source.receipt')}</span>
            <Input
              value={receiptId}
              disabled={state === 'running'}
              onChange={(event) => {
                generationIdRef.current += 1
                setReceiptId(event.target.value)
                setResult(null)
                setState(invocationActiveRef.current ? 'running' : 'idle')
              }}
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <Button disabled={!receiptId.trim() || state === 'running'} onClick={generate}>
            <FileSpreadsheet aria-hidden="true" className={cn(iconSize.sm, 'mr-2')} />
            {state === 'running' ? t('wbsXlsxDraft.actions.running') : t('wbsXlsxDraft.actions.generate')}
          </Button>
        </CardContent>
      </Card>

      {state === 'running' ? (
        <Alert variant="info" title={t('wbsXlsxDraft.states.running.title')}>
          {t('wbsXlsxDraft.states.running.body')}
        </Alert>
      ) : null}
      {state === 'error' ? (
        <Alert
          ref={statusRef}
          tabIndex={-1}
          variant="error"
          title={t('wbsXlsxDraft.states.error.title')}
          icon={<AlertTriangle className={iconSize.md} />}
        >
          {t('wbsXlsxDraft.states.error.body')}
        </Alert>
      ) : null}
      {result ? <ResultPanel result={result} statusRef={statusRef} /> : null}
    </main>
  )
}

function ResultPanel({ result, statusRef }: { result: GenerateWbsXlsxResult; statusRef: RefObject<HTMLDivElement> }) {
  const { t } = useTranslation()
  if (result.outcome === 'error') {
    const stateKey =
      result.error_kind === 'unsafe_workbook'
        ? 'unsafeWorkbook'
        : result.error_kind === 'local_receipt'
          ? 'localReceipt'
          : 'error'
    return (
      <Alert
        ref={statusRef}
        tabIndex={-1}
        variant="error"
        title={t(`wbsXlsxDraft.states.${stateKey}.title`)}
        icon={<AlertTriangle className={iconSize.md} />}
      >
        {t(`wbsXlsxDraft.states.${stateKey}.body`, { code: result.code })}
      </Alert>
    )
  }
  if (result.outcome === 'cancelled') {
    return (
      <Alert ref={statusRef} tabIndex={-1} variant="warning" title={t('wbsXlsxDraft.states.cancelled.title')}>
        {t('wbsXlsxDraft.states.cancelled.body')}
      </Alert>
    )
  }
  if (result.outcome === 'gate_rejected') {
    return (
      <Alert ref={statusRef} tabIndex={-1} variant="warning" title={t('wbsXlsxDraft.states.rejected.title')}>
        {t('wbsXlsxDraft.states.rejected.body', { reason: result.reason_code })}
      </Alert>
    )
  }
  if (result.outcome === 'header_drift') {
    return (
      <Alert ref={statusRef} tabIndex={-1} variant="warning" title={t('wbsXlsxDraft.states.headerDrift.title')}>
        {t('wbsXlsxDraft.states.headerDrift.body', { column: result.first_mismatch_column })}
      </Alert>
    )
  }
  return (
    <Card ref={statusRef} tabIndex={-1} data-testid="wbs-xlsx-produced">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <CheckCircle2 aria-hidden="true" className={cn(iconSize.md, 'text-semantic-success')} />
          {t('wbsXlsxDraft.states.produced.title')}
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <p className={typography.body}>{t('wbsXlsxDraft.states.produced.body')}</p>
        <dl className="grid gap-3 text-sm sm:grid-cols-2">
          <HashRow label={t('wbsXlsxDraft.fields.assignmentReceipt')} value={result.assignment_receipt_id} />
          <HashRow label={t('wbsXlsxDraft.fields.outputReceipt')} value={result.receipt_id} />
          <HashRow label={t('wbsXlsxDraft.fields.assignment')} value={result.assignment_id} />
          <HashRow label={t('wbsXlsxDraft.fields.mapping')} value={result.mapping_id} />
          <HashRow label={t('wbsXlsxDraft.fields.assignmentHash')} value={result.assignment_hash} />
          <HashRow label={t('wbsXlsxDraft.fields.sourceHash')} value={result.source_snapshot_hash} />
          <HashRow label={t('wbsXlsxDraft.fields.mappingHash')} value={result.mapping_content_hash} />
          <HashRow label={t('wbsXlsxDraft.fields.workbookStructureHash')} value={result.template_structure_hash} />
          <HashRow label={t('wbsXlsxDraft.fields.templateHash')} value={result.approved_template_hash} />
          <HashRow label={t('wbsXlsxDraft.fields.outputHash')} value={result.artifact_sha256} />
          <HashRow label={t('wbsXlsxDraft.fields.outputPath')} value={result.output_path} />
        </dl>
        <p className={cn(typography.caption, 'text-content-secondary')}>
          {result.receipt_upload_pending
            ? t('wbsXlsxDraft.states.produced.uploadPending')
            : t('wbsXlsxDraft.states.produced.uploaded')}
        </p>
      </CardContent>
    </Card>
  )
}

function HashRow({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt className="text-content-tertiary">{label}</dt>
      <dd className={cn(typography.family.mono, 'break-all text-content text-xs')}>{value}</dd>
    </div>
  )
}
