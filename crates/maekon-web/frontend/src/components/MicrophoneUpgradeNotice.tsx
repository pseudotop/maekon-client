import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { Alert, Button } from './ui'

/**
 * #4686: one-time banner shown when a returning user's microphone silently stopped
 * after the #4568 mic-consent split — i.e. `audio.enabled` is on but the new
 * `microphone` consent has not been granted (it used to ride on screen-capture
 * consent). The backend command `take_microphone_upgrade_notice` returns `true` at
 * most once (it records a one-time `app_meta` flag), so this banner appears a single
 * time and guides the user to the Privacy page to re-enable voice input.
 *
 * The frontend PULLS this on mount (command-based) rather than the backend pushing an
 * event from startup, which avoids the race where an event fires before the WebView
 * has registered its listener.
 */
export function MicrophoneUpgradeNotice() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [show, setShow] = useState(false)

  useEffect(() => {
    let cancelled = false
    import('@tauri-apps/api/core')
      .then(({ invoke }) => invoke<boolean>('take_microphone_upgrade_notice'))
      .then((shouldShow) => {
        if (!cancelled && shouldShow) setShow(true)
      })
      .catch((e) => {
        // Standalone / dev mode (no Tauri runtime) — silently skip.
        console.debug('take_microphone_upgrade_notice failed (standalone/dev mode):', e)
      })
    return () => {
      cancelled = true
    }
  }, [])

  if (!show) return null

  return (
    <Alert variant="warning" title={t('privacy.consent.microphone.upgradeNotice.title')} className="m-4">
      <p className="mb-3">{t('privacy.consent.microphone.upgradeNotice.body')}</p>
      <div className="flex gap-2">
        <Button
          variant="primary"
          size="sm"
          onClick={() => {
            navigate('/privacy')
            setShow(false)
          }}
        >
          {t('privacy.consent.microphone.upgradeNotice.cta')}
        </Button>
        <Button variant="ghost" size="sm" onClick={() => setShow(false)}>
          {t('privacy.consent.microphone.upgradeNotice.dismiss')}
        </Button>
      </div>
    </Alert>
  )
}
