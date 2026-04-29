import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { buildBugReportIssueUrl, buildMailtoUrl, createBugReport, formatBundleForClipboard } from '../api/bug-report'
import type { BugReportBundle } from '../api/contracts'
import { addToast } from '../hooks/useToast'
import { translateError, type WireErrorLocale } from '../i18n/translateError'
import { motion, typography } from '../styles/tokens'
import { IS_TAURI } from '../utils/platform'
import { Alert, Button, Card, CardTitle, Dialog, DialogBody, DialogContent, DialogFooter, DialogTitle } from './ui'

async function invokeDesktop<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

interface BugReportWizardProps {
  open: boolean
  onClose: () => void
}

type WizardStep = 'generate' | 'review' | 'share'

export default function BugReportWizard({ open, onClose }: BugReportWizardProps) {
  const { t, i18n } = useTranslation()

  const [step, setStep] = useState<WizardStep>('generate')
  const [piiLevel, setPiiLevel] = useState<string>('standard')
  const [includeLogs, setIncludeLogs] = useState(true)
  const [generating, setGenerating] = useState(false)
  const [bundle, setBundle] = useState<BugReportBundle | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [expandedSections, setExpandedSections] = useState<Record<string, boolean>>({})
  const [exporting, setExporting] = useState(false)

  const handleClose = useCallback(() => {
    setStep('generate')
    setBundle(null)
    setError(null)
    setExpandedSections({})
    setExporting(false)
    onClose()
  }, [onClose])

  const handleGenerate = useCallback(async () => {
    setGenerating(true)
    setError(null)
    try {
      const result = await createBugReport(includeLogs, piiLevel)
      setBundle(result)
      setStep('review')
    } catch (err) {
      const message = err instanceof Error ? err.message : t('settings.bugReportGenerateFailed')
      setError(message)
    } finally {
      setGenerating(false)
    }
  }, [includeLogs, piiLevel, t])

  const toggleSection = useCallback((key: string) => {
    setExpandedSections((prev) => ({ ...prev, [key]: !prev[key] }))
  }, [])

  const handleCopyBugId = useCallback(async () => {
    if (!bundle) return
    try {
      await navigator.clipboard.writeText(bundle.bug_id)
      addToast('success', t('settings.bugReportIdCopied'), 3000)
    } catch {
      addToast('error', t('settings.bugReportCopyFailed'), 4000)
    }
  }, [bundle, t])

  const handleOpenGitHub = useCallback(() => {
    if (!bundle) return
    const url = buildBugReportIssueUrl(bundle)
    const opened = window.open(url, '_blank', 'noopener,noreferrer')
    if (!opened) {
      addToast('error', t('settings.bugReportOpenFailed'), 5000)
    }
  }, [bundle, t])

  const handleCopyJson = useCallback(async () => {
    if (!bundle) return
    try {
      await navigator.clipboard.writeText(formatBundleForClipboard(bundle, 'json'))
      addToast('success', t('settings.bugReportJsonCopied'), 3000)
    } catch {
      addToast('error', t('settings.bugReportCopyFailed'), 4000)
    }
  }, [bundle, t])

  const handleCopyText = useCallback(async () => {
    if (!bundle) return
    try {
      await navigator.clipboard.writeText(formatBundleForClipboard(bundle, 'text'))
      addToast('success', t('settings.bugReportTextCopied'), 3000)
    } catch {
      addToast('error', t('settings.bugReportCopyFailed'), 4000)
    }
  }, [bundle, t])

  const handleExport = useCallback(async () => {
    if (!bundle) return
    setExporting(true)
    try {
      const result = await invokeDesktop<string | null>('export_bug_report', {
        bugId: bundle.bug_id,
        bundleJson: JSON.stringify(bundle),
      })
      if (result) {
        addToast('success', t('settings.bugReportExported'), 4000)
      }
      // null means user cancelled the save dialog — no toast needed
    } catch (error) {
      // ADR-019 Follow-up #3: use translateError so typed IpcError codes
      // (validation.invalid_arguments, internal.generic, internal.io) get
      // localized specific messages instead of a generic "export failed" toast.
      const locale = (i18n.language?.startsWith('ko') ? 'ko' : 'en') as WireErrorLocale
      const detail = translateError(error, locale)
      const message = detail ? `${t('settings.bugReportExportFailed')}: ${detail}` : t('settings.bugReportExportFailed')
      addToast('error', message, 5000)
    } finally {
      setExporting(false)
    }
  }, [bundle, t, i18n])

  const handleEmailSupport = useCallback(() => {
    if (!bundle) return
    const url = buildMailtoUrl(bundle.bug_id)
    const opened = window.open(url, '_blank', 'noopener,noreferrer')
    if (!opened) {
      addToast('error', t('settings.bugReportOpenFailed'), 5000)
    }
  }, [bundle, t])

  return (
    <Dialog open={open} onClose={handleClose}>
      <DialogContent size="lg" className="max-h-[85vh] overflow-hidden">
        <DialogTitle>{t('settings.bugReportWizardTitle')}</DialogTitle>
        <DialogBody className="max-h-[70vh] space-y-4 overflow-y-auto">
          <StepIndicator current={step} t={t} />

          {step === 'generate' && (
            <GenerateStep
              piiLevel={piiLevel}
              onPiiChange={setPiiLevel}
              includeLogs={includeLogs}
              onIncludeLogsChange={setIncludeLogs}
              generating={generating}
              error={error}
              onGenerate={handleGenerate}
              t={t}
            />
          )}

          {step === 'review' && bundle && (
            <ReviewStep
              bundle={bundle}
              expandedSections={expandedSections}
              onToggleSection={toggleSection}
              onCopyBugId={handleCopyBugId}
              t={t}
            />
          )}

          {step === 'share' && bundle && (
            <ShareStep
              onOpenGitHub={handleOpenGitHub}
              onCopyJson={handleCopyJson}
              onCopyText={handleCopyText}
              onExport={handleExport}
              onEmailSupport={handleEmailSupport}
              exporting={exporting}
              t={t}
            />
          )}
        </DialogBody>
        <DialogFooter>
          {step === 'review' && (
            <Button type="button" variant="ghost" size="sm" onClick={() => setStep('generate')}>
              {t('settings.bugReportBack')}
            </Button>
          )}
          {step === 'share' && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => {
                setExporting(false)
                setStep('review')
              }}
            >
              {t('settings.bugReportBack')}
            </Button>
          )}
          {step === 'review' && (
            <Button type="button" variant="primary" size="sm" onClick={() => setStep('share')}>
              {t('settings.bugReportNext')}
            </Button>
          )}
          <Button type="button" variant="ghost" size="sm" onClick={handleClose}>
            {t('common.close')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ── Step Indicator ── */

interface StepIndicatorProps {
  current: WizardStep
  t: (key: string) => string
}

const STEPS: WizardStep[] = ['generate', 'review', 'share']

function StepIndicator({ current, t }: StepIndicatorProps) {
  const labels: Record<WizardStep, string> = {
    generate: t('settings.bugReportStepGenerate'),
    review: t('settings.bugReportStepReview'),
    share: t('settings.bugReportStepShare'),
  }
  const currentIdx = STEPS.indexOf(current)

  return (
    <div className="flex items-center gap-2 text-xs">
      {STEPS.map((s, i) => (
        <div key={s} className="flex items-center gap-2">
          {i > 0 && <span className="text-content-muted">/</span>}
          <span className={i <= currentIdx ? `${typography.weight.medium} text-brand-text` : 'text-content-muted'}>
            {labels[s]}
          </span>
        </div>
      ))}
    </div>
  )
}

/* ── Step 1: Generate ── */

interface GenerateStepProps {
  piiLevel: string
  onPiiChange: (level: string) => void
  includeLogs: boolean
  onIncludeLogsChange: (value: boolean) => void
  generating: boolean
  error: string | null
  onGenerate: () => void
  t: (key: string) => string
}

function GenerateStep({
  piiLevel,
  onPiiChange,
  includeLogs,
  onIncludeLogsChange,
  generating,
  error,
  onGenerate,
  t,
}: GenerateStepProps) {
  return (
    <div className="space-y-4">
      <p className="text-content-secondary text-sm">{t('settings.bugReportGenerateDesc')}</p>

      <Card variant="default" padding="md" className="space-y-3">
        <CardTitle className="text-sm">{t('settings.bugReportPiiLevel')}</CardTitle>
        <div className="flex flex-col gap-2">
          <label className="flex cursor-pointer items-center gap-2 text-sm">
            <input
              type="radio"
              name="pii-level"
              value="standard"
              checked={piiLevel === 'standard'}
              onChange={() => onPiiChange('standard')}
              className="accent-brand"
            />
            <span className="text-content">{t('settings.bugReportPiiStandard')}</span>
          </label>
          <label className="flex cursor-pointer items-center gap-2 text-sm">
            <input
              type="radio"
              name="pii-level"
              value="strict"
              checked={piiLevel === 'strict'}
              onChange={() => onPiiChange('strict')}
              className="accent-brand"
            />
            <span className="text-content">{t('settings.bugReportPiiStrict')}</span>
          </label>
        </div>
      </Card>

      {IS_TAURI && (
        <Card variant="default" padding="md">
          <label className="flex cursor-pointer items-center gap-3">
            <input
              type="checkbox"
              checked={includeLogs}
              onChange={(e) => onIncludeLogsChange(e.target.checked)}
              className="accent-brand"
            />
            <div>
              <span className="text-content-strong text-sm">{t('settings.bugReportIncludeLogs')}</span>
              <p className="text-content-secondary text-xs">{t('settings.bugReportIncludeLogsDesc')}</p>
            </div>
          </label>
        </Card>
      )}

      {error && (
        <Alert variant="error" title={t('settings.bugReportGenerateFailedTitle')}>
          <p>{error}</p>
        </Alert>
      )}

      <Button type="button" variant="primary" size="sm" isLoading={generating} onClick={onGenerate}>
        {t('settings.bugReportGenerate')}
      </Button>
    </div>
  )
}

/* ── Step 2: Review ── */

interface ReviewStepProps {
  bundle: BugReportBundle
  expandedSections: Record<string, boolean>
  onToggleSection: (key: string) => void
  onCopyBugId: () => void
  t: (key: string) => string
}

function ReviewStep({ bundle, expandedSections, onToggleSection, onCopyBugId, t }: ReviewStepProps) {
  return (
    <div className="space-y-4">
      <Card variant="default" padding="md" className="flex items-center justify-between">
        <div>
          <p className="text-content-muted text-xs">{t('settings.bugReportBugId')}</p>
          <p className={`${typography.weight.bold} ${typography.family.mono} text-brand-text text-lg`}>
            {bundle.bug_id}
          </p>
        </div>
        <Button type="button" variant="secondary" size="sm" onClick={onCopyBugId}>
          {t('settings.bugReportCopyId')}
        </Button>
      </Card>

      <Card variant="default" padding="md" className="space-y-3">
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          <ReviewField label={t('settings.bugReportAppVersion')} value={bundle.system.app_version} />
          <ReviewField
            label={t('settings.bugReportOs')}
            value={`${bundle.system.os_name} ${bundle.system.os_version} (${bundle.system.arch})`}
          />
          <ReviewField label={t('settings.bugReportRuntime')} value={bundle.system.runtime} />
          <ReviewField label={t('settings.bugReportCpu')} value={`${bundle.system.cpu_count} cores`} />
          <ReviewField
            label={t('settings.bugReportMemory')}
            value={`${bundle.system.memory_available_mb} / ${bundle.system.memory_total_mb} MB`}
          />
          <ReviewField
            label={t('settings.bugReportStorageOk')}
            value={bundle.diagnostics.health.storage_ok ? t('settings.supportYes') : t('settings.supportNo')}
          />
        </div>
      </Card>

      <CollapsibleSection
        title={t('settings.bugReportConnectionTitle')}
        sectionKey="connection"
        expanded={expandedSections.connection ?? false}
        onToggle={onToggleSection}
      >
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          <ReviewField
            label={t('settings.bugReportServerReachable')}
            value={bundle.connection.server_reachable ? t('settings.supportYes') : t('settings.supportNo')}
          />
          <ReviewField
            label={t('settings.bugReportLastSync')}
            value={bundle.connection.last_sync_at ?? t('settings.bugReportNever')}
          />
          <ReviewField
            label={t('settings.bugReportGrpc')}
            value={bundle.connection.grpc_enabled ? t('settings.supportYes') : t('settings.supportNo')}
          />
          <ReviewField
            label={t('settings.bugReportWebSocket')}
            value={bundle.connection.websocket_connected ? t('settings.supportYes') : t('settings.supportNo')}
          />
        </div>
      </CollapsibleSection>

      <CollapsibleSection
        title={`${t('settings.bugReportAuditEntries')} (${bundle.diagnostics.recent_audit_entries.length})`}
        sectionKey="audit"
        expanded={expandedSections.audit ?? false}
        onToggle={onToggleSection}
      >
        {bundle.diagnostics.recent_audit_entries.length === 0 ? (
          <p className="text-content-muted text-xs">{t('settings.bugReportNoEntries')}</p>
        ) : (
          <pre className="max-h-40 overflow-auto rounded border border-DEFAULT bg-surface-base p-2 text-[11px] text-content-secondary">
            {bundle.diagnostics.recent_audit_entries
              .slice(0, 10)
              .map((e) => `[${e.timestamp}] ${e.action_type}: ${e.status}`)
              .join('\n')}
            {bundle.diagnostics.recent_audit_entries.length > 10
              ? `\n... and ${bundle.diagnostics.recent_audit_entries.length - 10} more`
              : ''}
          </pre>
        )}
      </CollapsibleSection>

      <CollapsibleSection
        title={`${t('settings.bugReportPolicyEvents')} (${bundle.diagnostics.recent_policy_events.length})`}
        sectionKey="policy"
        expanded={expandedSections.policy ?? false}
        onToggle={onToggleSection}
      >
        {bundle.diagnostics.recent_policy_events.length === 0 ? (
          <p className="text-content-muted text-xs">{t('settings.bugReportNoEntries')}</p>
        ) : (
          <pre className="max-h-40 overflow-auto rounded border border-DEFAULT bg-surface-base p-2 text-[11px] text-content-secondary">
            {bundle.diagnostics.recent_policy_events
              .slice(0, 10)
              .map((e) => `[${e.timestamp}] ${e.action_type}: ${e.status}`)
              .join('\n')}
            {bundle.diagnostics.recent_policy_events.length > 10
              ? `\n... and ${bundle.diagnostics.recent_policy_events.length - 10} more`
              : ''}
          </pre>
        )}
      </CollapsibleSection>
    </div>
  )
}

/* ── Step 3: Share ── */

interface ShareStepProps {
  onOpenGitHub: () => void
  onCopyJson: () => void
  onCopyText: () => void
  onExport: () => void
  onEmailSupport: () => void
  exporting: boolean
  t: (key: string) => string
}

function ShareStep({ onOpenGitHub, onCopyJson, onCopyText, onExport, onEmailSupport, exporting, t }: ShareStepProps) {
  return (
    <div className="space-y-4">
      <p className="text-content-secondary text-sm">{t('settings.bugReportShareDesc')}</p>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <ActionCard
          title={t('settings.bugReportOpenGitHub')}
          description={t('settings.bugReportOpenGitHubDesc')}
          onClick={onOpenGitHub}
        />
        <ActionCard
          title={t('settings.bugReportCopyJson')}
          description={t('settings.bugReportCopyJsonDesc')}
          onClick={onCopyJson}
        />
        <ActionCard
          title={t('settings.bugReportCopyText')}
          description={t('settings.bugReportCopyTextDesc')}
          onClick={onCopyText}
        />
        {IS_TAURI && (
          <ActionCard
            title={t('settings.bugReportExport')}
            description={t('settings.bugReportExportDesc')}
            onClick={onExport}
            loading={exporting}
          />
        )}
        <ActionCard
          title={t('settings.bugReportEmailSupport')}
          description={t('settings.bugReportEmailSupportDesc')}
          onClick={onEmailSupport}
        />
      </div>
    </div>
  )
}

/* ── Shared Sub-Components ── */

function ReviewField({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <p className="text-content-muted text-xs">{label}</p>
      <p className="text-content text-sm">{value}</p>
    </div>
  )
}

interface CollapsibleSectionProps {
  title: string
  sectionKey: string
  expanded: boolean
  onToggle: (key: string) => void
  children: React.ReactNode
}

function CollapsibleSection({ title, sectionKey, expanded, onToggle, children }: CollapsibleSectionProps) {
  return (
    <Card variant="default" padding="md" className="space-y-2">
      <button
        type="button"
        className="flex w-full items-center justify-between text-left"
        onClick={() => onToggle(sectionKey)}
      >
        <CardTitle className="text-sm">{title}</CardTitle>
        <span className="text-content-muted text-xs">{expanded ? '\u25B2' : '\u25BC'}</span>
      </button>
      {expanded && <div className="pt-2">{children}</div>}
    </Card>
  )
}

interface ActionCardProps {
  title: string
  description: string
  onClick: () => void
  loading?: boolean
}

function ActionCard({ title, description, onClick, loading }: ActionCardProps) {
  return (
    <button
      type="button"
      className={`rounded-lg border border-DEFAULT bg-surface p-4 text-left ${motion.colors} hover:bg-surface-hover`}
      onClick={onClick}
      disabled={loading}
    >
      <p className={`${typography.weight.medium} text-content-strong text-sm`}>{loading ? `${title}...` : title}</p>
      <p className="mt-1 text-content-secondary text-xs">{description}</p>
    </button>
  )
}
