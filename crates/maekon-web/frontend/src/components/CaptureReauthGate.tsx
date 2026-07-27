/**
 * #8044: capture-history viewing re-authentication gate component.
 *
 * Wraps sensitive capture-history pages (timeline/replay): if an enabled
 * gate is unauthenticated, the children (the data-fetching pages) are
 * **not mounted** and a re-auth prompt is shown instead — since children
 * never mount, frame fetches never happen prematurely. The backend
 * `require_capture_reauth` (403 auth.reauth_required) is the
 * defense-in-depth backstop.
 *
 * Fail-closed: a failed/cancelled auth attempt never opens viewing. When the
 * app goes to background (visibilitychange hidden) the gate re-locks,
 * requiring re-auth on the next foreground entry.
 */

import { createContext, type ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { isApiErrorCode } from '../api/client'
import {
  authenticateCaptureHistory,
  getCaptureReauthStatus,
  lockCaptureReauth,
  type ReauthOutcome,
  type ReauthStatus,
} from '../api/reauth'
import { isStandaloneModeEnabled } from '../api/standalone'
import { typography } from '../styles/tokens'
import { isTauriRuntime } from '../utils/platform'
import { Alert, Button, Card, CardContent, Dialog, DialogBody, DialogContent, DialogTitle, Input, Spinner } from './ui'

interface Props {
  children: ReactNode
}

type CaptureMutationRetry = () => void | Promise<void>

interface CaptureReauthRecoveryContextValue {
  /**
   * Handles the backend's idle-expiry backstop for an already-mounted
   * capture-history page. Returns `false` when the error is unrelated so the
   * caller can surface its normal mutation error.
   */
  requestCaptureReauth: (error: unknown, retry: CaptureMutationRetry) => Promise<boolean>
}

const CaptureReauthRecoveryContext = createContext<CaptureReauthRecoveryContextValue>({
  requestCaptureReauth: async () => false,
})

/** Recover a capture-history mutation rejected after its re-auth session expires. */
export function useCaptureReauthRecovery() {
  return useContext(CaptureReauthRecoveryContext)
}

interface CaptureReauthControlsProps {
  status: ReauthStatus
  onAuthenticated: () => void | Promise<void>
  onCancel: () => void
  onGoToSettings: () => void
}

/**
 * Reusable biometric/PIN controls for capture-history actions.
 *
 * Route gates and one-shot sensitive actions (for example, frame metadata
 * export) share the same fail-closed outcomes and PIN fallback instead of
 * implementing subtly different authentication behavior.
 */
export function CaptureReauthControls({
  status,
  onAuthenticated,
  onCancel,
  onGoToSettings,
}: CaptureReauthControlsProps) {
  const { t } = useTranslation()
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [pin, setPin] = useState('')
  const [pinMode, setPinMode] = useState(false)
  const mounted = useRef(true)

  useEffect(
    () => () => {
      mounted.current = false
    },
    [],
  )

  const applyOutcome = useCallback(
    async (outcome: ReauthOutcome) => {
      switch (outcome.outcome) {
        case 'authenticated':
          setError(null)
          await onAuthenticated()
          break
        case 'cancelled':
          setError(t('reauth.errors.cancelled'))
          break
        case 'unsupported':
          // No biometric support -> switch to the app PIN fallback.
          setPinMode(true)
          setError(t('reauth.errors.biometricUnavailable'))
          break
        case 'failed':
          setError(outcome.detail || t('reauth.errors.failed'))
          break
      }
    },
    [onAuthenticated, t],
  )

  const runBiometric = useCallback(async () => {
    setSubmitting(true)
    setError(null)
    try {
      const outcome = await authenticateCaptureHistory({
        method: 'biometric',
        reason: t('reauth.reason'),
      })
      await applyOutcome(outcome)
    } catch {
      setError(t('reauth.errors.failed'))
    } finally {
      if (mounted.current) setSubmitting(false)
    }
  }, [applyOutcome, t])

  const runPin = useCallback(async () => {
    if (!pin.trim()) return
    setSubmitting(true)
    setError(null)
    try {
      const outcome = await authenticateCaptureHistory({ method: 'pin', pin })
      await applyOutcome(outcome)
      if (mounted.current) setPin('')
    } catch {
      setError(t('reauth.errors.failed'))
    } finally {
      if (mounted.current) setSubmitting(false)
    }
  }, [applyOutcome, pin, t])

  const showBiometric = status.biometric_available && !pinMode
  const canPin = status.pin_enrolled

  return (
    <>
      {error && (
        <Alert variant="error" role="alert">
          {error}
        </Alert>
      )}

      {showBiometric ? (
        <div className="space-y-3">
          <Button
            type="button"
            variant="primary"
            className="w-full"
            isLoading={submitting}
            disabled={submitting}
            onClick={() => void runBiometric()}
          >
            {t('reauth.useBiometric', {
              kind: status.biometric_kind ?? t('reauth.biometricGeneric'),
            })}
          </Button>
          {canPin && (
            <Button
              type="button"
              variant="ghost"
              className="w-full"
              disabled={submitting}
              onClick={() => setPinMode(true)}
            >
              {t('reauth.usePinInstead')}
            </Button>
          )}
        </div>
      ) : canPin ? (
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault()
            void runPin()
          }}
        >
          <Input
            type="password"
            inputMode="numeric"
            autoComplete="off"
            aria-label={t('reauth.pinLabel')}
            placeholder={t('reauth.pinPlaceholder')}
            value={pin}
            onChange={(e) => setPin(e.target.value)}
            disabled={submitting}
            autoFocus
          />
          <Button
            type="submit"
            variant="primary"
            className="w-full"
            isLoading={submitting}
            disabled={submitting || !pin.trim()}
          >
            {t('reauth.unlock')}
          </Button>
          {status.biometric_available && (
            <Button
              type="button"
              variant="ghost"
              className="w-full"
              disabled={submitting}
              onClick={() => {
                setPinMode(false)
                setError(null)
              }}
            >
              {t('reauth.useBiometricInstead')}
            </Button>
          )}
        </form>
      ) : (
        <Alert variant="warning" title={t('reauth.noMethodTitle')}>
          {t('reauth.noMethodBody')}
        </Alert>
      )}

      <div className="flex items-center justify-between gap-3 pt-1">
        {!canPin && !status.biometric_available && (
          <Button type="button" variant="secondary" className="flex-1" onClick={onGoToSettings}>
            {t('reauth.goToSettings')}
          </Button>
        )}
        <Button type="button" variant="ghost" className="flex-1" disabled={submitting} onClick={onCancel}>
          {t('reauth.cancel')}
        </Button>
      </div>
    </>
  )
}

interface CaptureReauthDialogProps {
  status: ReauthStatus | null
  onAuthenticated: () => void | Promise<void>
  onClose: () => void
  onGoToSettings: () => void
}

/** Action-scoped re-auth dialog used by protected exports. */
export function CaptureReauthDialog({ status, onAuthenticated, onClose, onGoToSettings }: CaptureReauthDialogProps) {
  const { t } = useTranslation()

  return (
    <Dialog open={status !== null} onClose={onClose}>
      <DialogContent size="sm">
        <DialogTitle>{t('reauth.title')}</DialogTitle>
        <DialogBody className="space-y-4">
          <p>{t('reauth.description')}</p>
          {status && (
            <CaptureReauthControls
              status={status}
              onAuthenticated={onAuthenticated}
              onCancel={onClose}
              onGoToSettings={onGoToSettings}
            />
          )}
        </DialogBody>
      </DialogContent>
    </Dialog>
  )
}

export function CaptureReauthGate({ children }: Props) {
  const { t } = useTranslation()
  const navigate = useNavigate()

  const [status, setStatus] = useState<ReauthStatus | null>(null)
  const [recoveryStatus, setRecoveryStatus] = useState<ReauthStatus | null>(null)
  const [checking, setChecking] = useState(true)
  const mounted = useRef(true)
  const statusRef = useRef<ReauthStatus | null>(null)
  const pendingRetry = useRef<CaptureMutationRetry | null>(null)

  useEffect(() => {
    statusRef.current = status
  }, [status])

  const refreshStatus = useCallback(async () => {
    try {
      const next = await getCaptureReauthStatus()
      if (mounted.current) setStatus(next)
      return next
    } catch {
      // Handle a status-check failure. Outside a real Tauri runtime — the
      // standalone web-preview mode AND any non-Tauri host environment
      // (including jsdom/vitest, which every frontend-journey/unit test
      // renders under) — there is no real IPC backend for this gate to
      // protect at all, so it passes through. This is keyed on
      // `isTauriRuntime()` rather than the separate `isStandaloneModeEnabled()`
      // mock-data toggle, because that toggle defaults to "connected mode"
      // (false) even when there is no Tauri host to connect to (e.g. plain
      // browser preview or a test harness) — gating on it alone would
      // wrongly fail-closed in those environments.
      //
      // Inside the real Tauri desktop app, however, if the command
      // (rarely) fails, this is **fail-closed**: viewing stays blocked and
      // the re-auth prompt is shown (a subsequent successful
      // biometric/PIN attempt makes the next refreshStatus pass) — this
      // prevents a status-check failure from being mistaken for "viewing
      // allowed" and fail-opening the security gate.
      const passThrough = !isTauriRuntime() || isStandaloneModeEnabled()
      if (mounted.current) {
        setStatus(
          passThrough
            ? {
                enabled: false,
                idle_timeout_secs: 0,
                authenticated: true,
                biometric_available: false,
                biometric_kind: null,
                pin_enrolled: false,
              }
            : {
                enabled: true,
                idle_timeout_secs: 0,
                authenticated: false,
                biometric_available: true,
                biometric_kind: null,
                pin_enrolled: true,
              },
        )
      }
      return null
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    void (async () => {
      await refreshStatus()
      if (mounted.current) setChecking(false)
    })()
    return () => {
      mounted.current = false
    }
  }, [refreshStatus])

  const requestCaptureReauth = useCallback(async (error: unknown, retry: CaptureMutationRetry) => {
    if (!isApiErrorCode(error, 'auth.reauth_required')) return false

    pendingRetry.current = retry
    try {
      const next = await getCaptureReauthStatus()
      if (!mounted.current) return true

      if (!next.enabled || next.authenticated) {
        pendingRetry.current = null
        await retry()
      } else {
        setRecoveryStatus(next)
      }
    } catch {
      if (!mounted.current) return true

      // The rejected mutation already proves that the backend gate is
      // locked. Reuse the last known capabilities so a transient status
      // read cannot hide the recovery prompt.
      const lastKnown = statusRef.current
      setRecoveryStatus(
        lastKnown
          ? { ...lastKnown, enabled: true, authenticated: false }
          : {
              enabled: true,
              idle_timeout_secs: 0,
              authenticated: false,
              biometric_available: true,
              biometric_kind: null,
              pin_enrolled: true,
            },
      )
    }
    return true
  }, [])

  const recoveryContextValue = useMemo(() => ({ requestCaptureReauth }), [requestCaptureReauth])

  const closeRecovery = useCallback(() => {
    pendingRetry.current = null
    setRecoveryStatus(null)
  }, [])

  const completeRecovery = useCallback(async () => {
    const retry = pendingRetry.current
    pendingRetry.current = null
    setRecoveryStatus(null)
    setStatus((current) => (current ? { ...current, authenticated: true } : current))
    await retry?.()
  }, [])

  // Re-lock when the app goes to background → require re-auth on re-entry (Recall-style).
  useEffect(() => {
    function onVisibility() {
      if (document.hidden) {
        void lockCaptureReauth().catch(() => {})
      } else {
        void refreshStatus()
      }
    }
    document.addEventListener('visibilitychange', onVisibility)
    return () => document.removeEventListener('visibilitychange', onVisibility)
  }, [refreshStatus])

  // Still loading.
  if (checking || status === null) {
    return (
      <div className="flex h-full min-h-[240px] items-center justify-center">
        <Spinner />
      </div>
    )
  }

  // Gate disabled or already authenticated → render the page as-is.
  if (!status.enabled || status.authenticated) {
    return (
      <CaptureReauthRecoveryContext.Provider value={recoveryContextValue}>
        {children}
        <CaptureReauthDialog
          status={recoveryStatus}
          onAuthenticated={completeRecovery}
          onClose={closeRecovery}
          onGoToSettings={() => {
            closeRecovery()
            navigate('/settings/privacy')
          }}
        />
      </CaptureReauthRecoveryContext.Provider>
    )
  }

  // Re-auth required — the prompt panel (children stay unmounted to prevent early fetch).
  return (
    <div className="flex h-full min-h-[320px] items-center justify-center p-6">
      <Card className="w-full max-w-md">
        <CardContent className="space-y-4 p-6">
          <div className="space-y-1 text-center">
            <h2 className={typography.h3}>{t('reauth.title')}</h2>
            <p className="text-content-secondary text-sm">{t('reauth.description')}</p>
          </div>

          <CaptureReauthControls
            status={status}
            onAuthenticated={async () => {
              await refreshStatus()
            }}
            onCancel={() => navigate('/')}
            onGoToSettings={() => navigate('/settings/privacy')}
          />
        </CardContent>
      </Card>
    </div>
  )
}

/**
 * Wraps a route component in the re-auth gate (used from route-tree).
 * Children stay unmounted until authenticated, preventing an early
 * capture-history fetch.
 */
export function withCaptureReauthGate<P extends object>(Inner: React.ComponentType<P>): React.ComponentType<P> {
  function Guarded(props: P) {
    return (
      <CaptureReauthGate>
        <Inner {...props} />
      </CaptureReauthGate>
    )
  }
  Guarded.displayName = `withCaptureReauthGate(${Inner.displayName ?? Inner.name ?? 'Component'})`
  return Guarded
}
