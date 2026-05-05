import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type {
  FeatureCapabilitySnapshot,
  OAuthConnectionStatus,
  OAuthFlowStatus,
  ProviderSurfaceSpec,
  SecretBackendCapabilities,
} from '../../api/client'
import { oauthCancelFlow, oauthConnectionStatus, oauthFlowStatus, oauthRevoke, oauthStartFlow } from '../../api/client'
import { Alert, Badge, Button, Card } from '../../components/ui'
import {
  findFeatureCapability,
  maturityBadgeColor,
  providerSurfaceMaturity,
  providerSurfaceStatusCopyKey,
} from '../../features/featureCapabilities'
import { iconSize, typography } from '../../styles/tokens'
import { isOAuthPanelAvailable } from './oauth-panel-support'

type ExpiryLevel = 'ok' | 'warning' | 'critical' | 'none'

function getExpiryLevel(expiresAt: string | null | undefined): ExpiryLevel {
  if (!expiresAt) return 'none'
  const remaining = new Date(expiresAt).getTime() - Date.now()
  const minutes = remaining / 60_000
  if (minutes < 1) return 'critical'
  if (minutes <= 5) return 'warning'
  return 'ok'
}

const EXPIRY_BADGE_STYLES: Record<ExpiryLevel, string> = {
  ok: 'bg-semantic-success/20 text-semantic-success',
  warning: 'bg-semantic-warning/20 text-semantic-warning',
  critical: 'bg-semantic-error/20 text-semantic-error',
  none: 'bg-surface-muted text-content-tertiary',
}

type PanelState =
  | { phase: 'unavailable' }
  | { phase: 'loading' }
  | { phase: 'disconnected' }
  | { phase: 'reauth_required'; status: OAuthConnectionStatus }
  | { phase: 'connecting'; authUrl: string; flowId: string }
  | { phase: 'connected'; status: OAuthConnectionStatus }
  | { phase: 'error'; detail?: string; message: string }

const POLL_INTERVAL_MS = 1500
const POLL_TIMEOUT_MS = 120000

interface OAuthConnectionPanelProps {
  providerId: string
  providerName: string
  oauthSurface?: ProviderSurfaceSpec
  preferredCliSurface?: ProviderSurfaceSpec
  featureSnapshot?: FeatureCapabilitySnapshot | null
  secretBackendCapabilities?: SecretBackendCapabilities | null
  onUsePreferredCli?: () => void
}

export default function OAuthConnectionPanel({
  providerId,
  providerName,
  oauthSurface,
  preferredCliSurface,
  featureSnapshot,
  secretBackendCapabilities,
  onUsePreferredCli,
}: OAuthConnectionPanelProps) {
  const { t } = useTranslation()
  const [state, setState] = useState<PanelState>({ phase: 'loading' })
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const clearPoll = useCallback(() => {
    if (pollRef.current) {
      clearInterval(pollRef.current)
      pollRef.current = null
    }
  }, [])

  const toErrorState = useCallback(
    (error: unknown): PanelState => {
      const detail = error instanceof Error ? error.message : String(error)
      if (detail.includes('1455') || detail.includes('already in use')) {
        return { phase: 'error', detail, message: t('settingsOAuth.portConflict') }
      }
      if (detail.includes('not available') || detail.includes('unavailable')) {
        return { phase: 'error', detail, message: t('settingsOAuth.unavailable') }
      }
      return { phase: 'error', detail, message: t('settingsOAuth.genericError') }
    },
    [t],
  )

  const openAuthorizationPage = useCallback(
    (authUrl: string) => {
      const opened = window.open(authUrl, '_blank', 'noopener,noreferrer')
      if (!opened) {
        setState({ phase: 'error', message: t('settingsOAuth.openBrowserFailed') })
      }
    },
    [t],
  )

  const refreshStatus = useCallback(async () => {
    if (!isOAuthPanelAvailable()) {
      setState({ phase: 'unavailable' })
      return
    }
    try {
      if (!oauthSurface) {
        setState({ phase: 'error', message: t('settingsOAuth.unavailable') })
        return
      }

      if (!secretBackendCapabilities) {
        setState({ phase: 'loading' })
        return
      }

      const oauthFeature = findFeatureCapability(featureSnapshot, oauthSurface.surface_id)
      const statusCopyKey = providerSurfaceStatusCopyKey(oauthSurface, featureSnapshot)
      if (
        !secretBackendCapabilities.oauth_available ||
        !secretBackendCapabilities.os_secret_store_available ||
        oauthFeature?.availability === 'unavailable'
      ) {
        setState({
          phase: 'error',
          message: statusCopyKey != null ? t(statusCopyKey) : t('settingsOAuth.unavailable'),
        })
        return
      }
      const status = await oauthConnectionStatus(providerId)
      if (status.connected) {
        setState({ phase: 'connected', status })
      } else if (status.has_refresh_token) {
        setState({ phase: 'reauth_required', status })
      } else {
        setState({ phase: 'disconnected' })
      }
    } catch (err) {
      setState(toErrorState(err))
    }
  }, [featureSnapshot, oauthSurface, providerId, secretBackendCapabilities, t, toErrorState])

  const oauthFeature = oauthSurface ? findFeatureCapability(featureSnapshot, oauthSurface.surface_id) : null
  const preferredCliFeature = preferredCliSurface
    ? findFeatureCapability(featureSnapshot, preferredCliSurface.surface_id)
    : null
  const maturityLabel = t(`featureCapability.maturity.${providerSurfaceMaturity(oauthSurface, featureSnapshot)}`)
  const preferredCliMessage =
    preferredCliFeature?.status_copy_key != null ? t(preferredCliFeature.status_copy_key) : null
  const setupMessage = oauthFeature?.setup_copy_key ? t(oauthFeature.setup_copy_key) : null

  useEffect(() => {
    refreshStatus()
    return clearPoll
  }, [refreshStatus, clearPoll])

  // Auto-refresh status every 60s to update expiry badge
  useEffect(() => {
    if (state.phase !== 'connected') return
    const timer = setInterval(() => refreshStatus(), 60_000)
    return () => clearInterval(timer)
  }, [state.phase, refreshStatus])

  const handleConnect = useCallback(async () => {
    try {
      const handle = await oauthStartFlow(providerId)
      setState({ phase: 'connecting', authUrl: handle.auth_url, flowId: handle.flow_id })

      const startedAt = Date.now()

      // Poll for completion
      pollRef.current = setInterval(async () => {
        if (Date.now() - startedAt >= POLL_TIMEOUT_MS) {
          clearPoll()
          try {
            await oauthCancelFlow(handle.flow_id)
          } catch {
            // ignore timeout cleanup errors
          }
          setState({ phase: 'error', message: t('settingsOAuth.timeout') })
          return
        }

        try {
          const flowState: OAuthFlowStatus = await oauthFlowStatus(handle.flow_id)
          if (flowState.status === 'completed') {
            clearPoll()
            await refreshStatus()
          } else if (flowState.status === 'failed') {
            clearPoll()
            setState(toErrorState(flowState.error))
          } else if (flowState.status === 'cancelled') {
            clearPoll()
            setState({ phase: 'disconnected' })
          }
        } catch (err) {
          clearPoll()
          setState(toErrorState(err))
        }
      }, POLL_INTERVAL_MS)
    } catch (err) {
      setState(toErrorState(err))
    }
  }, [providerId, clearPoll, refreshStatus, t, toErrorState])

  const handleCancel = useCallback(async () => {
    if (state.phase === 'connecting') {
      clearPoll()
      try {
        await oauthCancelFlow(state.flowId)
      } catch {
        // ignore cancel errors
      }
      setState({ phase: 'disconnected' })
    }
  }, [state, clearPoll])

  const handleDisconnect = useCallback(async () => {
    try {
      await oauthRevoke(providerId)
      setState({ phase: 'disconnected' })
    } catch (err) {
      setState(toErrorState(err))
    }
  }, [providerId, toErrorState])

  return (
    <Card variant="default" padding="md" className="space-y-3">
      <div className="flex items-center gap-2">
        <h4 className={`${typography.weight.medium} text-content-strong text-sm`}>
          {providerName} {t('settingsOAuth.title')}
        </h4>
        <Badge color={oauthFeature ? maturityBadgeColor(oauthFeature.maturity) : 'warning'} size="sm">
          {maturityLabel}
        </Badge>
      </div>

      {preferredCliFeature?.preferred && preferredCliMessage && (
        <Alert variant="info">
          <div className="flex items-center gap-2">
            <Badge color="info" size="sm">
              {t('featureCapability.preferredPath')}
            </Badge>
            <span className="text-content-secondary text-xs">
              {t(`featureCapability.availability.${preferredCliFeature.availability}`)}
            </span>
          </div>
          <p className="mt-1 text-content-secondary text-xs">{preferredCliMessage}</p>
          {onUsePreferredCli && preferredCliSurface && (
            <Button type="button" variant="secondary" size="sm" className="mt-2" onClick={onUsePreferredCli}>
              {t('settingsOAuth.switchToCli', { surface: preferredCliSurface.display_name })}
            </Button>
          )}
        </Alert>
      )}

      {setupMessage && (
        <Alert variant="default" title={t('featureCapability.setupTitle')}>
          <p className="text-content-secondary text-xs">{setupMessage}</p>
          {oauthFeature?.configuration_env_vars && oauthFeature.configuration_env_vars.length > 0 && (
            <div className="space-y-1">
              <p className="text-content-muted text-xs">{t('featureCapability.setupEnvVars')}</p>
              <div className="flex flex-wrap gap-2">
                {oauthFeature.configuration_env_vars.map((envVar) => (
                  <Badge key={envVar} color="default" size="sm">
                    <code>{envVar}</code>
                  </Badge>
                ))}
              </div>
            </div>
          )}
          {oauthFeature?.setup_docs_url && (
            <a
              href={oauthFeature.setup_docs_url}
              target="_blank"
              rel="noreferrer"
              className="inline-flex text-accent text-xs underline"
            >
              {t('featureCapability.openSetupDocs')}
            </a>
          )}
        </Alert>
      )}

      {state.phase === 'loading' && <p className="text-content-secondary text-sm">{t('settingsOAuth.loading')}</p>}

      {state.phase === 'unavailable' && (
        <p className="text-content-secondary text-sm">{t('settingsOAuth.desktopOnly')}</p>
      )}

      {state.phase === 'disconnected' && (
        <div className="space-y-2">
          <p className="text-content-secondary text-sm">{t('settingsOAuth.disconnectedDescription')}</p>
          <Button type="button" variant="primary" size="sm" onClick={handleConnect}>
            {t('settingsOAuth.connect')}
          </Button>
        </div>
      )}

      {state.phase === 'reauth_required' && (
        <div className="space-y-2">
          <p className="text-content-secondary text-sm">{t('settingsOAuth.reauthDescription')}</p>
          {state.status.expires_at && (
            <p className="text-content-secondary text-xs">
              {t('settingsOAuth.expiresAt')}: {new Date(state.status.expires_at).toLocaleString()}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button type="button" variant="primary" size="sm" onClick={handleConnect}>
              {t('settingsOAuth.reauthRequired')}
            </Button>
            <Button type="button" variant="secondary" size="sm" onClick={handleDisconnect}>
              {t('settingsOAuth.disconnect')}
            </Button>
          </div>
        </div>
      )}

      {state.phase === 'connecting' && (
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <div className={`${iconSize.base} animate-spin rounded-full border-2 border-accent border-t-transparent`} />
            <p className="text-content-secondary text-sm">{t('settingsOAuth.connecting')}</p>
          </div>
          <p className="text-content-muted text-xs">{t('settingsOAuth.openBrowserHint')}</p>
          <div className="flex flex-wrap gap-2">
            <Button type="button" variant="primary" size="sm" onClick={() => openAuthorizationPage(state.authUrl)}>
              {t('settingsOAuth.openBrowser')}
            </Button>
            <Button type="button" variant="secondary" size="sm" onClick={handleCancel}>
              {t('settingsOAuth.cancel')}
            </Button>
          </div>
        </div>
      )}

      {state.phase === 'connected' && (
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <span className="h-2 w-2 rounded-full bg-status-connected" />
            <p className="text-content-secondary text-sm">{t('settingsOAuth.connected')}</p>
          </div>
          {state.status.expires_at && (
            <p className="text-content-secondary text-xs">
              {t('settingsOAuth.expiresAt')}: {new Date(state.status.expires_at).toLocaleString()}
            </p>
          )}
          {state.status.expires_at &&
            (() => {
              const level = getExpiryLevel(state.status.expires_at)
              // When no refresh token, treat warning-level expiry as critical
              const effectiveLevel = !state.status.has_refresh_token && level === 'warning' ? 'critical' : level
              if (effectiveLevel === 'ok') return null
              const label =
                effectiveLevel === 'critical' ? t('settingsOAuth.statusExpired') : t('settingsOAuth.statusExpiringSoon')
              return (
                <span
                  className={`inline-flex items-center rounded-full px-2 py-0.5 text-xs ${typography.weight.medium} ${EXPIRY_BADGE_STYLES[effectiveLevel]}`}
                >
                  {label}
                </span>
              )
            })()}
          {state.status.has_refresh_token === false && (
            <Alert variant="warning" className="py-1.5 text-xs">
              {t('settingsOAuth.noRefreshToken')}
            </Alert>
          )}
          <Button type="button" variant="secondary" size="sm" onClick={handleDisconnect}>
            {t('settingsOAuth.disconnect')}
          </Button>
        </div>
      )}

      {state.phase === 'error' && (
        <div className="space-y-2">
          <p className="text-semantic-error text-sm">{state.message}</p>
          {state.detail && <p className="text-content-muted text-xs">{state.detail}</p>}
          <Button type="button" variant="secondary" size="sm" onClick={refreshStatus}>
            {t('settingsOAuth.retry')}
          </Button>
        </div>
      )}

      <p className="text-content-muted text-xs">{t('settingsOAuth.disclaimer')}</p>
    </Card>
  )
}
