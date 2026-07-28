import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { AppSettings, UpdateChannel } from '../../api/client'
import LanguageSelector from '../../components/LanguageSelector'
import SupportToolsCard from '../../components/SupportToolsCard'
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
import { useTheme } from '../../contexts/ThemeContext'
import { addToast } from '../../hooks/useToast'
import { translateError, type WireErrorLocale } from '../../i18n/translateError'
import { form } from '../../styles/tokens'
import { IS_TAURI } from '../../utils/platform'
import { isSelectableUpdateChannel } from '../../utils/updateChannels'
import { useLoadedFormData, useSettingsFormContext } from '../settings/SettingsFormContext'
import NotificationSettings from './NotificationSettings'
import ScheduleSettings from './ScheduleSettings'
import ToggleRow from './ToggleRow'

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

// Exported for unit testing (Vitest).
// #8058 P2-4: theme control was reachable only from the command palette
// (CommandPalette.tsx). Surface it in Settings > General so the light/dark
// choice is discoverable. `useTheme().setTheme` already persists to localStorage
// and toggles the root `dark`/`light` class; first launch follows the OS
// `prefers-color-scheme` (ThemeContext.tsx).
export function AppearanceSection() {
  const { t } = useTranslation()
  const { theme, setTheme } = useTheme()

  return (
    <Card variant="default" padding="lg">
      <CardTitle sticky>{t('settings.appearanceTitle', 'Appearance')}</CardTitle>
      <ToggleRow
        label={t('common.darkMode', 'Dark mode')}
        description={t('settings.appearanceDarkModeDesc', 'Use a dark color theme across the dashboard.')}
        checked={theme === 'dark'}
        onChange={(checked) => setTheme(checked ? 'dark' : 'light')}
      />
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

      <AppearanceSection />

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
        <NotificationSettings
          notification={formData.notification}
          onChange={settingsForm.handleNotificationChange}
          onTestNotification={settingsForm.testNotificationMutation.mutate}
          isTestNotificationPending={settingsForm.testNotificationMutation.isPending}
        />
      </div>

      <div id="section-schedule">
        <ScheduleSettings
          schedule={formData.schedule}
          onChange={settingsForm.handleScheduleChange}
          // #7678: fail-closed while the capability snapshot query is still loading
          // (`undefined`) or unavailable outside Tauri, mirroring the `audioCompiled` gate.
          powerStatusAvailable={data.featureCapabilities?.power_status_available === true}
        />
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

      <AccountSection />

      <ViewSetupGuideButton />
      <SupportToolsCard />
    </div>
  )
}

/* ── Account: Sign out of all devices (OOS-TBD-N15-UI-EXPOSURE) ── */

// Exported for unit testing (Vitest).
export function AccountSection() {
  const { t, i18n } = useTranslation()
  const [confirmOpen, setConfirmOpen] = useState(false)
  const [loading, setLoading] = useState(false)

  const handleConfirm = useCallback(async () => {
    setLoading(true)
    try {
      await invokeDesktop<void>('logout_all_sessions')
      addToast('success', t('settings.account.success'))
      setConfirmOpen(false)
    } catch (e) {
      const lang = i18n.language?.split('-')[0]
      const locale: WireErrorLocale = lang === 'ko' ? 'ko' : 'en'
      const message = translateError(e, locale)
      addToast('error', t('settings.account.error', { error: message }))
    } finally {
      setLoading(false)
    }
  }, [i18n.language, t])

  return (
    <>
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.account.title')}</CardTitle>
        <div className="space-y-3">
          <p className="text-content-secondary text-sm">{t('settings.account.description')}</p>
          <p className="text-content-secondary text-xs">{t('settings.account.signOutAllDevicesDescription')}</p>
          <Button type="button" variant="danger" size="sm" onClick={() => setConfirmOpen(true)}>
            {t('settings.account.signOutAllDevicesButton')}
          </Button>
        </div>
      </Card>

      <Dialog open={confirmOpen} onClose={() => (loading ? undefined : setConfirmOpen(false))}>
        <DialogContent size="md">
          <DialogTitle>{t('settings.account.confirmTitle')}</DialogTitle>
          <DialogBody>
            <p className="text-content-secondary text-sm">{t('settings.account.confirmMessage')}</p>
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={() => setConfirmOpen(false)} disabled={loading}>
              {t('settings.account.cancel')}
            </Button>
            <Button type="button" variant="danger" size="sm" isLoading={loading} onClick={() => void handleConfirm()}>
              {t('settings.account.confirmAction')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
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
