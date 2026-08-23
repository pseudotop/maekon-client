import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { type DiagnosticsBundleResponse, fetchSupportDiagnostics } from '../api/client'
import { addToast } from '../hooks/useToast'
import { translateError, type WireErrorLocale } from '../i18n/translateError'
import { IS_TAURI } from '../utils/platform'
import BugReportWizard from './BugReportWizard'
import {
  Alert,
  Button,
  Card,
  CardTitle,
  Checkbox,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogTitle,
} from './ui'

// #8079: SupportToolsCard was previously a private component inside
// GeneralTab.tsx. It is extracted here so the dedicated /support destination
// (pages/support/SupportPage.tsx) and Settings → General can both render the
// same diagnostics + bug-report toolset from a single source.

const SUPPORT_DEVELOPER_DETAILS_KEY = 'maekon-support-developer-details'
const ALPHA_FEEDBACK_URL = 'https://maekon.dev/alpha-feedback'

interface RuntimeLogSnapshot {
  generated_at: string
  log_dir: string
  log_file: string | null
  line_count: number
  recent_text: string
}

function readDeveloperDetailsPreference(): boolean {
  if (typeof window === 'undefined') return import.meta.env.DEV
  return import.meta.env.DEV || window.localStorage.getItem(SUPPORT_DEVELOPER_DETAILS_KEY) === '1'
}

function persistDeveloperDetailsPreference(enabled: boolean) {
  if (typeof window === 'undefined') return
  window.localStorage.setItem(SUPPORT_DEVELOPER_DETAILS_KEY, enabled ? '1' : '0')
}

function formatTernary(value: boolean | null | undefined, yes: string, no: string, unknown: string): string {
  if (value === true) return yes
  if (value === false) return no
  return unknown
}

async function invokeDesktop<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

export function SupportToolsCard() {
  const { t, i18n } = useTranslation()
  const [open, setOpen] = useState(false)
  const [wizardOpen, setWizardOpen] = useState(false)
  const [loading, setLoading] = useState(false)
  const [diagnostics, setDiagnostics] = useState<DiagnosticsBundleResponse | null>(null)
  const [runtimeLogs, setRuntimeLogs] = useState<RuntimeLogSnapshot | null>(null)
  const [supportError, setSupportError] = useState<string | null>(null)
  const [logsError, setLogsError] = useState<string | null>(null)
  const [developerDetails, setDeveloperDetails] = useState(readDeveloperDetailsPreference)

  const loadSupportData = useCallback(
    async (includeLogs = developerDetails) => {
      setLoading(true)
      setSupportError(null)
      if (includeLogs) setLogsError(null)

      try {
        const [nextDiagnostics, nextLogs] = await Promise.all([
          fetchSupportDiagnostics(),
          includeLogs && IS_TAURI
            ? invokeDesktop<RuntimeLogSnapshot>('get_runtime_log_snapshot', { lineLimit: 200 }).catch((error) => {
                // ADR-019 Follow-up #3: route IpcError through translateError so
                // typed wire codes (internal.generic, storage.failed, etc.)
                // surface as localized user-facing messages rather than raw
                // Display strings. Falls through to the existing i18n key for
                // non-IpcError shapes (HTTP/network layer errors before the
                // command runs).
                const locale = (i18n.language?.startsWith('ko') ? 'ko' : 'en') as WireErrorLocale
                const message = translateError(error, locale) || t('settings.supportLogsLoadFailed')
                setLogsError(message)
                return null
              })
            : Promise.resolve(null),
        ])

        setDiagnostics(nextDiagnostics)
        if (includeLogs && IS_TAURI) {
          setRuntimeLogs(nextLogs)
        }
      } catch (error) {
        const message = error instanceof Error ? error.message : t('settings.supportLoadFailed')
        setSupportError(message)
      } finally {
        setLoading(false)
      }
    },
    [developerDetails, t, i18n],
  )

  useEffect(() => {
    if (!open) return
    void loadSupportData()
  }, [open, loadSupportData])

  const handleDeveloperToggle = useCallback(
    (enabled: boolean) => {
      setDeveloperDetails(enabled)
      persistDeveloperDetailsPreference(enabled)
      if (!enabled) {
        setRuntimeLogs(null)
        setLogsError(null)
        return
      }
      if (open) {
        void loadSupportData(true)
      }
    },
    [open, loadSupportData],
  )

  const handleCopyDiagnostics = useCallback(async () => {
    try {
      const snapshot = diagnostics ?? (await fetchSupportDiagnostics())
      setDiagnostics(snapshot)
      await navigator.clipboard.writeText(JSON.stringify(snapshot, null, 2))
      addToast('success', t('settings.supportDiagnosticsCopied'), 4000)
    } catch (error) {
      const message = error instanceof Error ? error.message : t('settings.supportCopyFailed')
      addToast('error', message, 5000)
    }
  }, [diagnostics, t])

  const handleCopyLogs = useCallback(async () => {
    try {
      const snapshot =
        runtimeLogs ??
        (IS_TAURI ? await invokeDesktop<RuntimeLogSnapshot>('get_runtime_log_snapshot', { lineLimit: 200 }) : null)
      if (!snapshot?.recent_text) {
        addToast('warning', t('settings.supportNoLogs'), 4000)
        return
      }
      setRuntimeLogs(snapshot)
      await navigator.clipboard.writeText(snapshot.recent_text)
      addToast('success', t('settings.supportLogsCopied'), 4000)
    } catch (error) {
      const message = error instanceof Error ? error.message : t('settings.supportCopyFailed')
      addToast('error', message, 5000)
    }
  }, [runtimeLogs, t])

  const handleOpenAlphaFeedback = useCallback(() => {
    const opened = window.open(ALPHA_FEEDBACK_URL, '_blank', 'noopener,noreferrer')
    if (!opened) {
      addToast('error', t('settings.supportOpenFeedbackFailed'), 5000)
    }
  }, [t])

  return (
    <>
      <Card variant="default" padding="lg">
        <div className="flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
          <div className="space-y-1">
            <CardTitle>{t('settings.supportTitle')}</CardTitle>
            <p className="text-content-secondary text-sm">{t('settings.supportDesc')}</p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button type="button" variant="secondary" size="sm" onClick={() => setOpen(true)}>
              {t('settings.supportOpenDetails')}
            </Button>
            <Button type="button" variant="secondary" size="sm" onClick={handleOpenAlphaFeedback}>
              {t('settings.supportAlphaFeedback')}
            </Button>
            <Button type="button" variant="primary" size="sm" onClick={() => setWizardOpen(true)}>
              {t('settings.supportReportBug')}
            </Button>
          </div>
        </div>
      </Card>

      <BugReportWizard open={wizardOpen} onClose={() => setWizardOpen(false)} />

      <Dialog open={open} onClose={() => setOpen(false)}>
        <DialogContent size="lg" className="max-h-[85vh] overflow-hidden">
          <DialogTitle>{t('settings.supportDialogTitle')}</DialogTitle>
          <DialogBody className="max-h-[70vh] space-y-4 overflow-y-auto">
            <p className="text-content-secondary text-sm">{t('settings.supportDialogDesc')}</p>

            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                variant="secondary"
                size="sm"
                isLoading={loading}
                onClick={() => void loadSupportData()}
              >
                {t('settings.supportRefresh')}
              </Button>
              <Button type="button" variant="secondary" size="sm" onClick={() => void handleCopyDiagnostics()}>
                {t('settings.supportCopyDiagnostics')}
              </Button>
            </div>

            <label className="flex cursor-pointer items-center gap-3">
              <Checkbox checked={developerDetails} onChange={(e) => handleDeveloperToggle(e.target.checked)} />
              <div>
                <span className="text-content-strong text-sm">{t('settings.supportDeveloperDetails')}</span>
                <p className="text-content-secondary text-xs">{t('settings.supportDeveloperDetailsDesc')}</p>
              </div>
            </label>

            {supportError && (
              <Alert variant="error" title={t('settings.supportLoadFailedTitle')}>
                <p>{supportError}</p>
              </Alert>
            )}

            {diagnostics && (
              <Card variant="default" padding="md" className="space-y-3">
                <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                  <div>
                    <p className="text-content-muted text-xs">{t('settings.supportSchemaVersion')}</p>
                    <p className="text-content text-sm">{diagnostics.schema_version}</p>
                  </div>
                  <div>
                    <p className="text-content-muted text-xs">{t('settings.supportGeneratedAt')}</p>
                    <p className="text-content text-sm">{diagnostics.generated_at}</p>
                  </div>
                  <div>
                    <p className="text-content-muted text-xs">{t('settings.supportStorageOk')}</p>
                    <p className="text-content text-sm">
                      {formatTernary(
                        diagnostics.health.storage_ok,
                        t('settings.supportYes'),
                        t('settings.supportNo'),
                        t('settings.supportUnknown'),
                      )}
                    </p>
                  </div>
                  <div>
                    <p className="text-content-muted text-xs">{t('settings.supportFramesDir')}</p>
                    <p className="text-content text-sm">
                      {formatTernary(
                        diagnostics.health.frames_dir_configured,
                        t('settings.supportYes'),
                        t('settings.supportNo'),
                        t('settings.supportUnknown'),
                      )}
                      {' · '}
                      {formatTernary(
                        diagnostics.health.frames_dir_exists,
                        t('settings.supportYes'),
                        t('settings.supportNo'),
                        t('settings.supportUnknown'),
                      )}
                    </p>
                  </div>
                  <div>
                    <p className="text-content-muted text-xs">{t('settings.supportAuditCount')}</p>
                    <p className="text-content text-sm">{diagnostics.recent_audit_entries.length}</p>
                  </div>
                  <div>
                    <p className="text-content-muted text-xs">{t('settings.supportPolicyCount')}</p>
                    <p className="text-content text-sm">{diagnostics.recent_policy_events.length}</p>
                  </div>
                </div>
                <div>
                  <p className="text-content-muted text-xs">{t('settings.supportProviderCli')}</p>
                  {(diagnostics.provider_cli ?? []).length === 0 ? (
                    <p className="text-content-secondary text-sm">{t('settings.supportProviderCliEmpty')}</p>
                  ) : (
                    <div className="mt-2 space-y-2">
                      {(diagnostics.provider_cli ?? []).map((entry) => (
                        <div key={entry.surface_id} className="rounded border border-DEFAULT bg-surface-base p-2">
                          <p className="text-content text-sm">
                            {entry.surface_id}: {entry.readiness} / {entry.availability}
                          </p>
                          {entry.dependency_status && (
                            <p className="text-content-secondary text-xs">
                              {t('settings.supportProviderCliDependency')}: {entry.dependency_status}
                            </p>
                          )}
                          {entry.env_refresh_required && (
                            <p className="text-content-secondary text-xs">
                              {t('settings.supportProviderCliRestartRequired')}
                            </p>
                          )}
                          {entry.status_reason && (
                            <p className="text-content-secondary text-xs">{entry.status_reason}</p>
                          )}
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </Card>
            )}

            {developerDetails && (
              <Card variant="default" padding="md" className="space-y-3">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <CardTitle className="mb-1">{t('settings.supportRecentLogs')}</CardTitle>
                    <p className="text-content-secondary text-xs">
                      {runtimeLogs?.log_file
                        ? `${t('settings.supportLogSource')}: ${runtimeLogs.log_file}`
                        : t('settings.supportNoLogs')}
                    </p>
                  </div>
                  <Button type="button" variant="secondary" size="sm" onClick={() => void handleCopyLogs()}>
                    {t('settings.supportCopyLogs')}
                  </Button>
                </div>

                {logsError && (
                  <Alert variant="warning" title={t('settings.supportLogsUnavailable')}>
                    <p>{logsError}</p>
                  </Alert>
                )}

                <pre className="max-h-72 overflow-auto rounded-md border border-DEFAULT bg-surface-base p-3 text-[11px] text-content-secondary">
                  {runtimeLogs?.recent_text || t('settings.supportNoLogs')}
                </pre>
              </Card>
            )}
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={() => setOpen(false)}>
              {t('common.close')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}

export default SupportToolsCard
