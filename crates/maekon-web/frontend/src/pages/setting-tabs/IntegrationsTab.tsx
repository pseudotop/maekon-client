import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  acknowledgeIntegrationPrompt,
  cancelIntegrationDeviceAuthorization,
  dismissIntegrationPrompt,
  fetchIntegrationAuthStatus,
  fetchIntegrationInbox,
  fetchIntegrationStatus,
  type IntegrationAuthStatus,
  type IntegrationDeviceAuthorizationFlow,
  type IntegrationInboxPromptSummary,
  pollIntegrationDeviceAuthorization,
  refreshIntegrationInbox,
  resetIntegrationAuth,
  startIntegrationDeviceAuthorization,
} from '../../api/client'
import { Alert, Badge, Button, Card, CardTitle, Spinner } from '../../components/ui'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

// #8080: Integration lifecycle surface — inbox (receipt / ack / dismiss),
// device authorization (status → start → poll → connected), and a
// reset/disconnect affordance wiring the existing /integration/auth/reset
// endpoint. All UI is composed from api-client functions that were previously
// dead-wired (no consumer).

export default function IntegrationsTab() {
  const { t } = useTranslation()
  const {
    data: status,
    isLoading,
    isError,
    refetch,
  } = useQuery({
    queryKey: ['integration-status'],
    queryFn: fetchIntegrationStatus,
    staleTime: 30_000,
    retry: 1,
  })

  if (isLoading) {
    return (
      <div className="flex items-center gap-2">
        <Spinner size="sm" />
        <span className={cn('text-sm', colors.text.tertiary)}>{t('integrations.loading')}</span>
      </div>
    )
  }

  if (isError || !status) {
    return (
      <div className="space-y-4">
        <h2 className={cn(typography.h2, colors.text.primary)}>{t('integrations.title')}</h2>
        <Alert variant="error" title={t('integrations.statusError')}>
          <Button type="button" variant="secondary" size="sm" className="mt-2" onClick={() => void refetch()}>
            {t('common.retry')}
          </Button>
        </Alert>
      </div>
    )
  }

  // Outbound integration auth/inbox lifecycle is independent from the inbound
  // web.allow_external switch. The latter only controls whether the local Web
  // API binds beyond loopback and must never be required just to authorize an
  // outbound integration from the desktop UI.
  if (!status.outbound_runtime?.enabled) {
    return (
      <div className="space-y-4">
        <h2 className={cn(typography.h2, colors.text.primary)}>{t('integrations.title')}</h2>
        <p className={cn('text-sm', colors.text.secondary)}>{t('integrations.description')}</p>
        <div className={cn('rounded-lg border p-4', colors.surface.muted)}>
          <p className={cn('text-sm', typography.weight.semibold, colors.text.primary)}>
            {t('integrations.disabledTitle')}
          </p>
          <p className={cn('mt-1 text-sm', colors.text.secondary)}>{t('integrations.disabledDescription')}</p>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <div className="space-y-1">
        <h2 className={cn(typography.h2, colors.text.primary)}>{t('integrations.title')}</h2>
        <p className={cn('text-sm', colors.text.secondary)}>{t('integrations.description')}</p>
      </div>

      <DeviceAuthSection />
      <InboxSection />
    </div>
  )
}

// ── Device authorization ──────────────────────────────────────

type AuthPhase =
  | { phase: 'loading' }
  | { phase: 'error'; message: string }
  | { phase: 'idle'; status: IntegrationAuthStatus }
  | { phase: 'authorizing'; flow: IntegrationDeviceAuthorizationFlow; status: IntegrationAuthStatus }
  | { phase: 'connected'; status: IntegrationAuthStatus }

function pollIntervalMs(flow: IntegrationDeviceAuthorizationFlow): number {
  return Math.max(1000, (flow.interval_secs || 5) * 1000)
}

function isFlowExpired(flow: IntegrationDeviceAuthorizationFlow): boolean {
  const expiry = new Date(flow.expires_at).getTime()
  return Number.isFinite(expiry) && Date.now() > expiry
}

function DeviceAuthSection() {
  const { t } = useTranslation()
  const [state, setState] = useState<AuthPhase>({ phase: 'loading' })
  const [busy, setBusy] = useState(false)
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const clearPoll = useCallback(() => {
    if (pollRef.current) {
      clearInterval(pollRef.current)
      pollRef.current = null
    }
  }, [])

  const startPolling = useCallback(
    (flow: IntegrationDeviceAuthorizationFlow) => {
      clearPoll()
      pollRef.current = setInterval(async () => {
        if (isFlowExpired(flow)) {
          clearPoll()
          setState({ phase: 'error', message: t('integrations.auth.expired') })
          return
        }
        try {
          const result = await pollIntegrationDeviceAuthorization({ flow_id: flow.flow_id })
          if (result.auth_status.authenticated) {
            clearPoll()
            setState({ phase: 'connected', status: result.auth_status })
          } else if (result.auth_status.status === 'error') {
            clearPoll()
            setState({ phase: 'error', message: result.auth_status.message ?? t('integrations.auth.error') })
          } else if (!result.flow && !result.auth_status.pending_flow) {
            clearPoll()
            setState({ phase: 'idle', status: result.auth_status })
          }
        } catch {
          clearPoll()
          setState({ phase: 'error', message: t('integrations.auth.error') })
        }
      }, pollIntervalMs(flow))
    },
    [clearPoll, t],
  )

  const applyStatus = useCallback(
    (next: IntegrationAuthStatus) => {
      if (next.authenticated) {
        setState({ phase: 'connected', status: next })
      } else if (next.pending_flow) {
        setState({ phase: 'authorizing', flow: next.pending_flow, status: next })
        startPolling(next.pending_flow)
      } else {
        setState({ phase: 'idle', status: next })
      }
    },
    [startPolling],
  )

  const loadStatus = useCallback(async () => {
    try {
      const next = await fetchIntegrationAuthStatus()
      applyStatus(next)
    } catch {
      setState({ phase: 'error', message: t('integrations.auth.error') })
    }
  }, [applyStatus, t])

  useEffect(() => {
    void loadStatus()
    return clearPoll
  }, [loadStatus, clearPoll])

  const handleConnect = useCallback(async () => {
    setBusy(true)
    try {
      const result = await startIntegrationDeviceAuthorization()
      if (result.flow) {
        setState({ phase: 'authorizing', flow: result.flow, status: result.auth_status })
        startPolling(result.flow)
      } else {
        applyStatus(result.auth_status)
      }
    } catch {
      setState({ phase: 'error', message: t('integrations.auth.startError') })
    } finally {
      setBusy(false)
    }
  }, [applyStatus, startPolling, t])

  const handleCancel = useCallback(
    async (flow: IntegrationDeviceAuthorizationFlow) => {
      setBusy(true)
      clearPoll()
      try {
        await cancelIntegrationDeviceAuthorization({ flow_id: flow.flow_id })
      } catch {
        // Best-effort cancel — reload the true status regardless.
      }
      await loadStatus()
      setBusy(false)
    },
    [clearPoll, loadStatus],
  )

  const handleDisconnect = useCallback(async () => {
    setBusy(true)
    clearPoll()
    try {
      const result = await resetIntegrationAuth()
      applyStatus(result.auth_status)
    } catch {
      setState({ phase: 'error', message: t('integrations.auth.error') })
    } finally {
      setBusy(false)
    }
  }, [applyStatus, clearPoll, t])

  return (
    <Card variant="default" padding="lg" className="space-y-3">
      <div>
        <CardTitle>{t('integrations.auth.title')}</CardTitle>
        <p className="mt-1 text-content-secondary text-sm">{t('integrations.auth.description')}</p>
      </div>

      {state.phase === 'loading' && (
        <div className="flex items-center gap-2">
          <Spinner size="sm" />
          <span className="text-content-secondary text-sm">{t('integrations.auth.loading')}</span>
        </div>
      )}

      {state.phase === 'error' && (
        <div className="space-y-2">
          <p className="text-semantic-error text-sm">{state.message}</p>
          <Button type="button" variant="secondary" size="sm" onClick={() => void loadStatus()}>
            {t('common.retry')}
          </Button>
        </div>
      )}

      {state.phase === 'idle' && !state.status.interactive && (
        <p className="text-content-secondary text-sm">{t('integrations.auth.nonInteractive')}</p>
      )}

      {state.phase === 'idle' && state.status.interactive && (
        <div className="space-y-2">
          <p className="text-content-secondary text-sm">{t('integrations.auth.notConnected')}</p>
          <Button type="button" variant="primary" size="sm" isLoading={busy} onClick={() => void handleConnect()}>
            {t('integrations.auth.connect')}
          </Button>
        </div>
      )}

      {state.phase === 'authorizing' && (
        <div className="space-y-3">
          <p className="text-content-secondary text-sm">{t('integrations.auth.awaiting')}</p>
          <div className={cn('rounded-lg border p-4', colors.surface.muted)}>
            <p className="text-content-secondary text-sm">{t('integrations.auth.instructions')}</p>
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <span className="text-content-muted text-xs">{t('integrations.auth.codeLabel')}</span>
              <code
                className={cn(
                  'rounded bg-surface-base px-2 py-1 text-content text-sm tracking-widest',
                  typography.family.mono,
                )}
              >
                {state.flow.user_code}
              </code>
            </div>
            <a
              href={state.flow.verification_uri_complete ?? state.flow.verification_uri}
              target="_blank"
              rel="noreferrer"
              className="mt-3 inline-flex text-accent text-sm underline"
            >
              {t('integrations.auth.openVerification')}
            </a>
            <p className="mt-2 text-content-muted text-xs">
              {t('integrations.auth.expiresAt')}: {new Date(state.flow.expires_at).toLocaleString()}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Spinner size="sm" />
            <Button
              type="button"
              variant="secondary"
              size="sm"
              isLoading={busy}
              onClick={() => void handleCancel(state.flow)}
            >
              {t('integrations.auth.cancel')}
            </Button>
          </div>
        </div>
      )}

      {state.phase === 'connected' && (
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <span className="h-2 w-2 rounded-full bg-status-connected" />
            <p className="text-content-secondary text-sm">{t('integrations.auth.connected')}</p>
          </div>
          {state.status.expires_at && (
            <p className="text-content-secondary text-xs">
              {t('integrations.auth.expiresAt')}: {new Date(state.status.expires_at).toLocaleString()}
            </p>
          )}
          <p className="text-content-muted text-xs">{t('integrations.auth.disconnectHint')}</p>
          <Button type="button" variant="secondary" size="sm" isLoading={busy} onClick={() => void handleDisconnect()}>
            {t('integrations.auth.disconnect')}
          </Button>
        </div>
      )}
    </Card>
  )
}

// ── Inbox ─────────────────────────────────────────────────────

function InboxSection() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [actionError, setActionError] = useState<string | null>(null)

  const { data, isLoading, isError, refetch } = useQuery({
    queryKey: ['integration-inbox'],
    queryFn: fetchIntegrationInbox,
    staleTime: 15_000,
    retry: 1,
  })

  const invalidate = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: ['integration-inbox'] })
  }, [queryClient])

  const refreshMutation = useMutation({
    mutationFn: refreshIntegrationInbox,
    onSuccess: invalidate,
    onError: () => setActionError(t('integrations.inbox.error')),
  })

  const ackMutation = useMutation({
    mutationFn: (promptId: string) => acknowledgeIntegrationPrompt(promptId),
    onSuccess: invalidate,
    onError: () => setActionError(t('integrations.inbox.actionError')),
  })

  const dismissMutation = useMutation({
    mutationFn: (promptId: string) => dismissIntegrationPrompt(promptId, {}),
    onSuccess: invalidate,
    onError: () => setActionError(t('integrations.inbox.actionError')),
  })

  const prompts = data?.prompts ?? []
  const actionPending = ackMutation.isPending || dismissMutation.isPending

  return (
    <Card variant="default" padding="lg" className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <CardTitle>{t('integrations.inbox.title')}</CardTitle>
          <p className="mt-1 text-content-secondary text-sm">{t('integrations.inbox.description')}</p>
        </div>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          isLoading={refreshMutation.isPending}
          onClick={() => {
            setActionError(null)
            refreshMutation.mutate()
          }}
        >
          {t('integrations.inbox.refresh')}
        </Button>
      </div>

      {actionError && !isError && (
        <Alert variant="error" title={actionError}>
          <span />
        </Alert>
      )}

      {isLoading && (
        <div className="flex items-center gap-2">
          <Spinner size="sm" />
          <span className="text-content-secondary text-sm">{t('integrations.inbox.loading')}</span>
        </div>
      )}

      {isError && (
        <div className="space-y-2">
          <p className="text-semantic-error text-sm">{t('integrations.inbox.error')}</p>
          <Button type="button" variant="secondary" size="sm" onClick={() => void refetch()}>
            {t('common.retry')}
          </Button>
        </div>
      )}

      {!isLoading && !isError && prompts.length === 0 && (
        <p className="text-content-secondary text-sm">{t('integrations.inbox.empty')}</p>
      )}

      {prompts.length > 0 && (
        <div className="space-y-2">
          {prompts.map((prompt) => (
            <InboxPromptRow
              key={prompt.prompt_id}
              prompt={prompt}
              disabled={actionPending}
              onAck={() => {
                setActionError(null)
                ackMutation.mutate(prompt.prompt_id)
              }}
              onDismiss={() => {
                setActionError(null)
                dismissMutation.mutate(prompt.prompt_id)
              }}
            />
          ))}
        </div>
      )}
    </Card>
  )
}

interface InboxPromptRowProps {
  prompt: IntegrationInboxPromptSummary
  disabled: boolean
  onAck: () => void
  onDismiss: () => void
}

function InboxPromptRow({ prompt, disabled, onAck, onDismiss }: InboxPromptRowProps) {
  const { t } = useTranslation()
  const actionable = prompt.status === 'pending'

  return (
    <div className={cn('rounded-lg border p-3', colors.surface.elevated)}>
      <div className="flex flex-wrap items-center gap-2">
        <Badge color="primary" size="sm">
          {prompt.category}
        </Badge>
        <Badge color="info" size="sm">
          {prompt.priority}
        </Badge>
        <Badge color={actionable ? 'warning' : 'default'} size="sm">
          {prompt.status}
        </Badge>
      </div>
      <p className={cn('mt-2 text-sm', typography.weight.medium, colors.text.primary)}>{prompt.title}</p>
      {prompt.body && <p className={cn('mt-1 text-sm', colors.text.secondary)}>{prompt.body}</p>}
      <div className="mt-2 flex flex-wrap items-center gap-3 text-content-tertiary text-xs">
        <span>
          {t('integrations.inbox.source')}: {prompt.source_system}
        </span>
        <span>
          {t('integrations.inbox.received')}: {new Date(prompt.received_at).toLocaleString()}
        </span>
      </div>
      {actionable && (
        <div className="mt-3 flex flex-wrap gap-2">
          <Button type="button" variant="primary" size="sm" disabled={disabled} onClick={onAck}>
            {t('integrations.inbox.ack')}
          </Button>
          <Button type="button" variant="secondary" size="sm" disabled={disabled} onClick={onDismiss}>
            {t('integrations.inbox.dismiss')}
          </Button>
        </div>
      )}
    </div>
  )
}
