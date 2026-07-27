/**
 * #8044: capture-history re-authentication settings section (PrivacyTab).
 *
 * Self-contained — managed via dedicated IPC commands, separate from the
 * config form:
 * - Enabled + idle expiry: `set_capture_reauth_config` (persists to config +
 *   applies to the live gate).
 * - App PIN fallback enroll/remove: `register/clear_capture_reauth_pin`
 *   (app_meta, not config).
 * Status is always read from `get_capture_reauth_status` (the live gate) to
 * keep a single source of truth.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  clearCaptureReauthPin,
  getCaptureReauthStatus,
  type ReauthStatus,
  registerCaptureReauthPin,
  setCaptureReauthConfig,
} from '../../api/reauth'
import { Alert, Button, Card, CardTitle, Input } from '../../components/ui'
import { addToast } from '../../hooks/useToast'
import { typography } from '../../styles/tokens'
import ToggleRow from './ToggleRow'

const MIN_PIN_LEN = 4

export default function CaptureReauthSettings() {
  const { t } = useTranslation()
  const [status, setStatus] = useState<ReauthStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [pinOpen, setPinOpen] = useState(false)
  const [newPin, setNewPin] = useState('')
  const [confirmPin, setConfirmPin] = useState('')
  const [pinError, setPinError] = useState<string | null>(null)
  const mounted = useRef(true)

  const refresh = useCallback(async () => {
    try {
      const next = await getCaptureReauthStatus()
      if (mounted.current) setStatus(next)
    } catch {
      // Tauri unavailable (standalone) — hide the settings UI itself (renders null below).
      if (mounted.current) setStatus(null)
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    void refresh()
    return () => {
      mounted.current = false
    }
  }, [refresh])

  const applyConfig = useCallback(
    async (enabled: boolean, idleSecs: number) => {
      setBusy(true)
      try {
        await setCaptureReauthConfig(enabled, idleSecs)
        await refresh()
      } catch {
        addToast('error', t('reauth.settings.saveFailed'))
      } finally {
        if (mounted.current) setBusy(false)
      }
    },
    [refresh, t],
  )

  const savePin = useCallback(async () => {
    setPinError(null)
    if (newPin.length < MIN_PIN_LEN) {
      setPinError(t('reauth.settings.pinTooShort'))
      return
    }
    if (newPin !== confirmPin) {
      setPinError(t('reauth.settings.pinMismatch'))
      return
    }
    setBusy(true)
    try {
      await registerCaptureReauthPin(newPin)
      addToast('success', t('reauth.settings.pinSaved'))
      setPinOpen(false)
      setNewPin('')
      setConfirmPin('')
      await refresh()
    } catch {
      setPinError(t('reauth.errors.failed'))
    } finally {
      if (mounted.current) setBusy(false)
    }
  }, [confirmPin, newPin, refresh, t])

  const removePin = useCallback(async () => {
    setBusy(true)
    try {
      await clearCaptureReauthPin()
      addToast('success', t('reauth.settings.pinRemoved'))
      await refresh()
    } catch {
      addToast('error', t('reauth.errors.failed'))
    } finally {
      if (mounted.current) setBusy(false)
    }
  }, [refresh, t])

  // The standard (non-Tauri) environment has no re-auth feature, so don't render this section.
  if (status === null) return null

  const idleMinutes = Math.max(1, Math.round(status.idle_timeout_secs / 60))

  return (
    <Card variant="default" padding="lg" id="section-reauth">
      <CardTitle>{t('reauth.settings.sectionTitle')}</CardTitle>
      <p className="mb-4 text-content-secondary text-sm">{t('reauth.settings.sectionDescription')}</p>

      <div className="space-y-4">
        <ToggleRow
          label={t('reauth.settings.enableLabel')}
          description={t('reauth.settings.enableHint')}
          checked={status.enabled}
          disabled={busy}
          onChange={(checked) => void applyConfig(checked, status.idle_timeout_secs)}
        />

        {status.enabled && (
          <label className="flex items-center justify-between gap-4">
            <span className="text-sm">{t('reauth.settings.idleLabel')}</span>
            <Input
              type="number"
              min={1}
              max={60}
              className="w-24"
              value={idleMinutes}
              disabled={busy}
              onChange={(e) => {
                const minutes = Number.parseInt(e.target.value, 10)
                if (Number.isFinite(minutes) && minutes > 0) {
                  void applyConfig(status.enabled, minutes * 60)
                }
              }}
            />
          </label>
        )}

        <div className="rounded-md border border-border p-3">
          <p className={typography.label}>{t('reauth.settings.pinSectionTitle')}</p>
          <p className="mt-1 text-content-secondary text-xs">
            {status.biometric_available
              ? t('reauth.settings.biometricAvailable', { kind: status.biometric_kind ?? '' })
              : t('reauth.settings.biometricUnavailable')}
          </p>
          <p className="mt-1 text-content-secondary text-xs">
            {status.pin_enrolled ? t('reauth.settings.pinRegistered') : t('reauth.settings.pinNotRegistered')}
          </p>

          {pinOpen ? (
            <div className="mt-3 space-y-2">
              {pinError && (
                <Alert variant="error" role="alert">
                  {pinError}
                </Alert>
              )}
              <Input
                type="password"
                inputMode="numeric"
                autoComplete="new-password"
                aria-label={t('reauth.settings.pinNewLabel')}
                placeholder={t('reauth.settings.pinNewLabel')}
                value={newPin}
                disabled={busy}
                onChange={(e) => setNewPin(e.target.value)}
              />
              <Input
                type="password"
                inputMode="numeric"
                autoComplete="new-password"
                aria-label={t('reauth.settings.pinConfirmLabel')}
                placeholder={t('reauth.settings.pinConfirmLabel')}
                value={confirmPin}
                disabled={busy}
                onChange={(e) => setConfirmPin(e.target.value)}
              />
              <div className="flex gap-2">
                <Button type="button" variant="primary" isLoading={busy} disabled={busy} onClick={() => void savePin()}>
                  {t('reauth.settings.save')}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => {
                    setPinOpen(false)
                    setPinError(null)
                    setNewPin('')
                    setConfirmPin('')
                  }}
                >
                  {t('reauth.settings.cancel')}
                </Button>
              </div>
            </div>
          ) : (
            <div className="mt-3 flex gap-2">
              <Button type="button" variant="secondary" disabled={busy} onClick={() => setPinOpen(true)}>
                {status.pin_enrolled ? t('reauth.settings.changePin') : t('reauth.settings.registerPin')}
              </Button>
              {status.pin_enrolled && (
                <Button type="button" variant="ghost" disabled={busy} onClick={() => void removePin()}>
                  {t('reauth.settings.removePin')}
                </Button>
              )}
            </div>
          )}
        </div>
      </div>
    </Card>
  )
}
