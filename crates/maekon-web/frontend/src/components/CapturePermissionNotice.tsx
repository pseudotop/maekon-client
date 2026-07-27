import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchDesktopPermissionStatus, openDesktopPermissionSettings } from '../api/client'
import { setOsCapturePermissionBlocked, useOsCapturePermissionBlocked } from '../hooks/useOsPermissionStatus'
import { Alert, Button } from './ui'

/**
 * #8686 AC4: persistent recovery banner shown while the OS screen-recording
 * permission is revoked mid-session. The Rust monitor loop has already
 * stopped capture fail-closed and pushed the `capture-os-permission` event;
 * this banner gives the re-grant path (System Settings deep link) and a
 * re-check that clears the banner once the permission is granted again —
 * without waiting for the next revoke/restore event edge.
 *
 * Unlike `MicrophoneUpgradeNotice` this banner is NOT dismissable: capture
 * the user believes is running is silently off, so the guidance must stay
 * until the permission is actually restored or the user disables capture.
 */
export function CapturePermissionNotice() {
  const { t } = useTranslation()
  const blocked = useOsCapturePermissionBlocked()
  const [checking, setChecking] = useState(false)

  if (!blocked) return null

  const handleOpenSettings = () => {
    openDesktopPermissionSettings('screen_capture').catch((e) => {
      console.debug('openDesktopPermissionSettings failed (standalone/dev mode):', e)
    })
  }

  const handleRecheck = async () => {
    setChecking(true)
    try {
      const snapshot = await fetchDesktopPermissionStatus()
      if (snapshot.screen_capture.state === 'granted') {
        setOsCapturePermissionBlocked(false)
      }
    } catch (e) {
      console.debug('fetchDesktopPermissionStatus failed (standalone/dev mode):', e)
    } finally {
      setChecking(false)
    }
  }

  return (
    <Alert
      variant="error"
      title={t('capturePermission.bannerTitle')}
      className="m-4"
      data-testid="capture-permission-notice"
    >
      <p className="mb-3">{t('capturePermission.bannerBody')}</p>
      <div className="flex gap-2">
        <Button variant="primary" size="sm" onClick={handleOpenSettings}>
          {t('capturePermission.openSettings')}
        </Button>
        <Button variant="ghost" size="sm" onClick={handleRecheck} disabled={checking}>
          {checking ? t('capturePermission.rechecking') : t('capturePermission.recheck')}
        </Button>
      </div>
    </Alert>
  )
}
