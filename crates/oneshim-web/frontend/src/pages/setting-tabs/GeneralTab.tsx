import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  type AppSettings,
  type DiagnosticsBundleResponse,
  fetchSupportDiagnostics,
  type UpdateChannel,
} from '../../api/client'
import BugReportWizard from '../../components/BugReportWizard'
import LanguageSelector from '../../components/LanguageSelector'
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
  Input,
} from '../../components/ui'
import { DEFAULT_WEB_PORT } from '../../constants'
import { addToast } from '../../hooks/useToast'
import { translateError, type WireErrorLocale } from '../../i18n/translateError'
import { form } from '../../styles/tokens'
import { IS_TAURI } from '../../utils/platform'
import { isSelectableUpdateChannel } from '../../utils/updateChannels'
import { useLoadedFormData, useSettingsFormContext } from '../settings/SettingsFormContext'
import NotificationSettings from './NotificationSettings'
import ScheduleSettings from './ScheduleSettings'
import ToggleRow from './ToggleRow'

const SUPPORT_DEVELOPER_DETAILS_KEY = 'oneshim-support-developer-details'

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

interface AutostartCapabilities {
  supported: boolean
  unsupported_reason?: { kind: string }
  environment: string
}

// Exported for unit testing (Vitest).
export function StartupSection() {
  const { t } = useTranslation()
  const [enabled, setEnabled] = useState<boolean | null>(null)
  const [caps, setCaps] = useState<AutostartCapabilities | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    Promise.all([
      invokeDesktop<boolean>('is_autostart_enabled'),
      invokeDesktop<AutostartCapabilities>('autostart_capabilities'),
    ])
      .then(([e, c]) => {
        setEnabled(e)
        setCaps(c)
      })
      .catch((e) => setError(String(e)))
  }, [])

  const handleToggle = async (next: boolean) => {
    if (loading) return
    setLoading(true)
    setError(null)
    try {
      await invokeDesktop(next ? 'enable_autostart' : 'disable_autostart')
      setEnabled(next)
    } catch (e) {
      setError(String(e))
      const actual = await invokeDesktop<boolean>('is_autostart_enabled').catch(() => null)
      if (actual !== null) setEnabled(actual)
    } finally {
      setLoading(false)
    }
  }

  const isDisabled = loading || enabled === null || (caps !== null && !caps.supported)

  return (
    <Card variant="default" padding="lg">
      <CardTitle sticky>{t('settings.autostart.title')}</CardTitle>
      <div className="space-y-3">
        <p className="text-content-secondary text-sm">{t('settings.autostart.description')}</p>
        <label className="flex cursor-pointer items-center gap-3">
          <Checkbox
            checked={enabled ?? false}
            onChange={(e) => void handleToggle(e.target.checked)}
            disabled={isDisabled}
          />
          <span className="text-content-strong text-sm">{t('settings.autostart.toggle')}</span>
        </label>
        {caps && !caps.supported && (
          <p className="text-content-secondary text-xs">
            {t('settings.autostart.unsupported', {
              context: caps.unsupported_reason?.kind ?? 'unknown',
            })}
          </p>
        )}
        {error && (
          <Alert variant="error" title={t('settings.autostart.error', { error })}>
            <span />
          </Alert>
        )}
      </div>
    </Card>
  )
}

export default function GeneralTab() {
  const { form: settingsForm, data } = useSettingsFormContext()
  const { t } = useTranslation()
  const formData = useLoadedFormData()

  return (
    <div className="space-y-6">
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.language')}</CardTitle>
        <LanguageSelector />
      </Card>

      <StartupSection />

      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.webTitle')}</CardTitle>
        <div className="grid grid-cols-1 gap-6 md:grid-cols-2">
          <div>
            <label htmlFor="settings-web-port" className={form.label}>
              {t('settings.portLabel')}
            </label>
            <Input
              id="settings-web-port"
              type="number"
              min={1024}
              max={65535}
              value={formData.web_port}
              onChange={(e) =>
                settingsForm.handleRootChange(
                  'web_port' as keyof AppSettings,
                  parseInt(e.target.value, 10) || DEFAULT_WEB_PORT,
                )
              }
            />
            <p className={form.helper}>{t('settings.portRestart')}</p>
          </div>
          <div className="flex items-center">
            <div>
              <label className="flex cursor-pointer items-center">
                <Checkbox
                  checked={formData.allow_external}
                  onChange={(e) =>
                    settingsForm.handleRootChange('allow_external' as keyof AppSettings, e.target.checked)
                  }
                  className="mr-3"
                />
                <div>
                  <span className="text-content-strong">{t('settings.allowExternal')}</span>
                  <p className="text-content-secondary text-xs">{t('settings.allowExternalDesc')}</p>
                </div>
              </label>
              <p className={`${form.helper} mt-2`}>{t('settings.allowExternalIntegrationOnly')}</p>
            </div>
          </div>
        </div>
      </Card>

      <div id="section-notification">
        <NotificationSettings notification={formData.notification} onChange={settingsForm.handleNotificationChange} />
      </div>

      <div id="section-schedule">
        <ScheduleSettings schedule={formData.schedule} onChange={settingsForm.handleScheduleChange} />
      </div>

      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.updateTitle')}</CardTitle>
        <div className="space-y-4">
          <ToggleRow
            label={t('settings.updateEnabled')}
            description={t('settings.updateEnabledDesc')}
            checked={formData.update.enabled}
            onChange={(value) => settingsForm.handleUpdateChange('enabled', value)}
          />

          <ToggleRow
            label={t('settings.updateAutoInstall')}
            description={t('settings.updateAutoInstallDesc')}
            checked={formData.update.auto_install}
            onChange={(value) => settingsForm.handleUpdateChange('auto_install', value)}
          />

          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <div>
              <label htmlFor="settings-update-interval" className={form.label}>
                {t('settings.updateIntervalHours')}
              </label>
              <Input
                id="settings-update-interval"
                type="number"
                min={1}
                max={168}
                value={formData.update.check_interval_hours}
                onChange={(e) =>
                  settingsForm.handleUpdateChange('check_interval_hours', parseInt(e.target.value, 10) || 24)
                }
              />
            </div>
            <div className="flex items-end">
              <div>
                <label className="mb-1 block text-content-strong text-sm" htmlFor="update-channel">
                  {t('settings.updateChannel', 'Update channel')}
                </label>
                <select
                  id="update-channel"
                  value={formData.update.channel ?? 'stable'}
                  onChange={(e) => {
                    const channel = e.target.value as UpdateChannel
                    if (isSelectableUpdateChannel(channel)) settingsForm.handleUpdateChange('channel', channel)
                  }}
                  className="rounded-md border border-border bg-surface px-3 py-1.5 text-content text-sm"
                >
                  <option value="stable">{t('settings.channelStable', 'Stable')}</option>
                  <option value="pre_release">{t('settings.channelPreRelease', 'Pre-release (RC)')}</option>
                  <option value="nightly" disabled>
                    {t('settings.channelNightlyUnavailable')}
                  </option>
                </select>
                <p className="mt-1 text-content-secondary text-xs">
                  {isSelectableUpdateChannel(formData.update.channel ?? 'stable')
                    ? t('settings.updateChannelDesc', 'Choose which releases to receive')
                    : t('settings.updateChannelUnavailable')}
                </p>
              </div>
            </div>
          </div>

          <Alert variant="info" title={t('settings.updateRuntimeStatus')} className="mt-2">
            <div className="mt-1 text-content-strong text-sm">
              {data.updateStatus?.message ?? t('settings.updateStatusUnavailable')}
            </div>
            {data.updateStatus?.pending && (
              <div className="mt-2 space-y-1 text-content-secondary text-xs">
                <div>
                  {t('settings.updateCurrentVersion')}: {data.updateStatus.pending.current_version}
                </div>
                <div>
                  {t('settings.updateLatestVersion')}: {data.updateStatus.pending.latest_version}
                </div>
                <a
                  href={data.updateStatus.pending.release_url}
                  target="_blank"
                  rel="noreferrer"
                  className="text-brand-text underline"
                >
                  {t('settings.updateReleaseNote')}
                </a>
              </div>
            )}
            <div className="mt-4 flex flex-wrap gap-2">
              <Button
                type="button"
                variant="secondary"
                size="sm"
                isLoading={settingsForm.updateActionMutation.isPending}
                onClick={() => settingsForm.updateActionMutation.mutate('CheckNow')}
              >
                {t('settings.updateCheckNow')}
              </Button>
              <Button
                type="button"
                variant="primary"
                size="sm"
                isLoading={settingsForm.updateActionMutation.isPending}
                onClick={() => settingsForm.updateActionMutation.mutate('Approve')}
              >
                {t('settings.updateApproveNow')}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                isLoading={settingsForm.updateActionMutation.isPending}
                onClick={() => settingsForm.updateActionMutation.mutate('Defer')}
              >
                {t('settings.updateDefer')}
              </Button>
            </div>
          </Alert>
        </div>
      </Card>

      <ViewSetupGuideButton />
      <SupportToolsCard />
    </div>
  )
}

/* ── View Setup Guide (reset onboarding) ── */

function ViewSetupGuideButton() {
  const { t } = useTranslation()
  const [loading, setLoading] = useState(false)

  const handleClick = useCallback(async () => {
    setLoading(true)
    try {
      if (IS_TAURI) {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('reset_onboarding')
      }
      // Both Tauri and standalone: reload triggers onboarding check in App.tsx
      window.location.reload()
    } catch {
      // Standalone / dev mode — no Tauri runtime
      setLoading(false)
    }
  }, [])

  return (
    <Card variant="default" padding="lg">
      <div className="flex items-center justify-between">
        <div>
          <CardTitle className="mb-1">{t('settings.viewSetupGuide')}</CardTitle>
          <p className="text-content-secondary text-sm">{t('settings.viewSetupGuideDesc')}</p>
        </div>
        <Button type="button" variant="secondary" size="sm" isLoading={loading} onClick={handleClick}>
          {t('settings.viewSetupGuide')}
        </Button>
      </div>
    </Card>
  )
}

function SupportToolsCard() {
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
                      {diagnostics.health.frames_dir_path ?? t('settings.supportUnknown')}
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
