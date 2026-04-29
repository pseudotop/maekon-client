import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchUpdateStatus, postUpdateAction, type UpdateAction, type UpdateStatus } from '../api/client'
import { useUpdateStream } from '../hooks/useUpdateStream'
import { motion, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { Badge, Button, Card, CardTitle, Spinner } from './ui'

type UpdatePanelProps = {
  compact?: boolean
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

/**
 * Phase 4 D11: format a release `published_at` ISO-8601 timestamp as a
 * short human-readable string. Returns `null` when input is null or
 * unparseable so callers can render a fallback.
 *
 * - Less than 24h old: relative ("3 hours ago" / "5 minutes ago" / "12 days ago")
 * - Older: absolute ISO YYYY-MM-DD
 */
function formatReleaseDate(
  iso: string | null | undefined,
  nowMs: number,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string | null {
  if (iso == null || iso === '') return null
  const ts = Date.parse(iso)
  if (!Number.isFinite(ts)) return null

  const diffMs = nowMs - ts
  if (diffMs < 0) {
    // Future timestamp (clock skew); fall back to ISO date.
    return iso.slice(0, 10)
  }

  const minutes = Math.floor(diffMs / 60_000)
  if (minutes < 60) {
    return t('updates.releasedAgoMinutes', { count: Math.max(minutes, 1) })
  }
  const hours = Math.floor(minutes / 60)
  if (hours < 24) {
    return t('updates.releasedAgoHours', { count: hours })
  }
  const days = Math.floor(hours / 24)
  if (days < 7) {
    return t('updates.releasedAgoDays', { count: days })
  }
  // >= 7 days: absolute ISO date (YYYY-MM-DD) is cleaner than "N weeks ago".
  return iso.slice(0, 10)
}

export default function UpdatePanel({ compact = false }: UpdatePanelProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const stream = useUpdateStream()
  const [nowMs, setNowMs] = useState(() => Date.now())

  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 5000)
    return () => window.clearInterval(timer)
  }, [])

  const { data, isLoading } = useQuery<UpdateStatus>({
    queryKey: ['update-status'],
    queryFn: fetchUpdateStatus,
    refetchInterval: stream.status === 'connected' ? false : 15000,
    retry: 1,
  })

  const status = useMemo(() => stream.latest ?? data, [stream.latest, data])

  const freshness = useMemo(() => {
    if (!status?.updated_at) {
      return { ageSec: null, stale: true, severelyStale: true }
    }

    const timestamp = Date.parse(status.updated_at)
    if (!Number.isFinite(timestamp)) {
      return { ageSec: null, stale: true, severelyStale: true }
    }

    const ageSec = Math.max(0, Math.floor((nowMs - timestamp) / 1000))
    return {
      ageSec,
      stale: ageSec > 15,
      severelyStale: ageSec > 60,
    }
  }, [nowMs, status?.updated_at])

  const actionMutation = useMutation({
    mutationFn: (action: UpdateAction) => postUpdateAction(action),
    onSuccess: (response) => {
      queryClient.setQueryData(['update-status'], response.status)
      queryClient.invalidateQueries({ queryKey: ['update-status'] })
    },
  })

  const canApproveOrDefer =
    (status?.phase === 'PendingApproval' || status?.phase === 'ReadyToInstall') && !freshness.severelyStale
  const hasApprovalAction = status?.phase === 'PendingApproval' || status?.phase === 'ReadyToInstall'
  const isDownloading = status?.phase === 'Downloading'

  const phaseLabel = useMemo(() => {
    const phase = status?.phase
    if (phase === 'PendingApproval') return t('updates.pendingApproval')
    if (phase === 'Downloading') return t('updates.downloading', 'Downloading...')
    if (phase === 'ReadyToInstall') return t('updates.readyToInstall', 'Ready to Install')
    if (phase === 'Installing') return t('updates.installing')
    if (phase === 'Updated') return t('updates.updated')
    if (phase === 'Deferred') return t('updates.deferred')
    if (phase === 'Error') return t('updates.error')
    if (phase === 'Checking') return t('updates.checking')
    if (phase === 'RolledBack') return t('updates.rolledBack')
    return t('updates.idle')
  }, [status?.phase, t])

  if (isLoading && !status) {
    return (
      <Card variant="default" padding="lg">
        <div className="flex items-center space-x-2">
          <Spinner size="sm" />
          <span className="text-content-secondary">{t('updates.loading')}</span>
        </div>
      </Card>
    )
  }

  return (
    <Card variant="default" padding="lg">
      <div className="mb-4 flex items-center justify-between">
        <CardTitle>{t('updates.title')}</CardTitle>
        <div className="flex items-center space-x-2">
          <Badge color={stream.status === 'connected' ? 'success' : 'warning'} size="sm">
            {stream.status === 'connected' ? t('updates.live') : t('updates.polling')}
          </Badge>
          <Badge color={freshness.stale ? 'warning' : 'success'} size="sm">
            {freshness.stale ? t('updates.stale') : t('updates.fresh')}
          </Badge>
          <Badge
            color={
              status?.phase === 'Error' || status?.phase === 'RolledBack'
                ? 'error'
                : status?.phase === 'PendingApproval'
                  ? 'warning'
                  : status?.phase === 'Downloading'
                    ? 'info'
                    : status?.phase === 'ReadyToInstall'
                      ? 'success'
                      : 'info'
            }
            size="sm"
          >
            {phaseLabel}
          </Badge>
        </div>
      </div>

      <p className={cn('text-content-strong', typography.body)}>{status?.message ?? t('updates.statusUnavailable')}</p>

      <p className={cn('mt-2 text-content-secondary', typography.caption)}>
        {freshness.ageSec === null
          ? t('updates.lastUpdateUnknown')
          : t('updates.lastUpdateAge', { seconds: freshness.ageSec })}
      </p>

      {hasApprovalAction && freshness.severelyStale && (
        <div className={cn('mt-2 text-semantic-warning', typography.caption)}>{t('updates.staleActionBlocked')}</div>
      )}

      {(stream.lastError || stream.retryCount > 0) && (
        <div className={cn('mt-2 text-semantic-warning', typography.caption)}>
          {stream.lastError ? t('updates.streamIssue') : t('updates.reconnecting', { count: stream.retryCount })}
        </div>
      )}

      {status?.phase === 'RolledBack' && status.rollback && (
        <div
          className={cn(
            'mt-3 space-y-2 rounded-lg border border-semantic-error bg-surface-secondary p-3 text-content-strong',
            typography.body,
          )}
        >
          <div className={cn(typography.h3)}>{t('updates.rolledBackTitle')}</div>
          <div className={cn(typography.caption, 'text-content-secondary')}>
            {status.rollback.from_published_at && status.rollback.to_published_at
              ? t('updates.rolledBackFromToWithDates', {
                  fromVersion: status.rollback.from_version,
                  fromDate:
                    formatReleaseDate(status.rollback.from_published_at, nowMs, t) ??
                    status.rollback.from_published_at.slice(0, 10),
                  toVersion: status.rollback.to_version,
                  toDate:
                    formatReleaseDate(status.rollback.to_published_at, nowMs, t) ??
                    status.rollback.to_published_at.slice(0, 10),
                })
              : t('updates.rolledBackFromTo', {
                  fromVersion: status.rollback.from_version,
                  toVersion: status.rollback.to_version,
                })}
          </div>
          <div className={cn(typography.caption)}>{t('updates.rolledBackReasonRepeatedStartupFailure')}</div>
          <div className={cn(typography.caption, 'text-content-tertiary')}>
            {t('updates.rolledBackAt', { timestamp: status.rollback.rolled_back_at })}
          </div>
        </div>
      )}

      {status?.pending && (
        <div
          className={cn('mt-3 space-y-1 rounded-lg border border-muted p-3 text-content-secondary', typography.caption)}
        >
          <div>
            {t('updates.currentVersion')}: {status.pending.current_version}
          </div>
          <div>
            {(() => {
              const relDate = formatReleaseDate(status.pending.published_at, nowMs, t)
              return relDate
                ? `${t('updates.latestVersion')}: ${status.pending.latest_version} (${relDate})`
                : `${t('updates.latestVersion')}: ${status.pending.latest_version}`
            })()}
          </div>
          <a href={status.pending.release_url} target="_blank" rel="noreferrer" className="text-brand-text underline">
            {t('updates.openRelease')}
          </a>
          {status.pending.download_size_bytes != null && status.pending.download_size_bytes > 0 && (
            <p className={cn('mt-1', typography.caption, 'text-content-tertiary')}>
              {t('updates.downloadSize', { size: formatBytes(status.pending.download_size_bytes) })}
            </p>
          )}
          {status.pending.release_notes && (
            <details className="mt-3">
              <summary className={cn('cursor-pointer select-none', typography.caption, 'text-content-secondary')}>
                {t('updates.releaseNotes')}
              </summary>
              <pre
                className={cn(
                  'mt-2 max-h-64 overflow-y-auto whitespace-pre-wrap rounded-md bg-surface-secondary p-3',
                  typography.caption,
                )}
              >
                {status.pending.release_notes}
              </pre>
            </details>
          )}
        </div>
      )}

      {status?.phase === 'Downloading' && status.download_progress && (
        <div className="mt-3 space-y-1">
          <div className="h-2 w-full rounded-full bg-surface-secondary">
            <div
              className={cn('h-full rounded-full bg-brand-text', motion.all)}
              style={{ width: `${Math.min(status.download_progress.percent, 100)}%` }}
            />
          </div>
          <p className={cn(typography.caption, 'text-text-tertiary')}>
            {formatBytes(status.download_progress.bytes_downloaded)} /{' '}
            {formatBytes(status.download_progress.total_bytes)} ({status.download_progress.percent.toFixed(1)}%)
          </p>
        </div>
      )}

      {status?.phase === 'ReadyToInstall' && (
        <p className={cn('mt-2', typography.body, 'text-semantic-success')}>
          {t('updates.readyToInstallMsg', 'Download complete. Ready to install.')}
        </p>
      )}

      {!compact && (
        <div className={cn('mt-3 text-content-secondary', typography.caption)}>
          {status?.auto_install ? t('updates.autoInstallOn') : t('updates.autoInstallOff')}
        </div>
      )}

      <div className="mt-4 flex flex-wrap gap-2">
        <Button
          type="button"
          variant="secondary"
          size="sm"
          isLoading={actionMutation.isPending}
          disabled={isDownloading}
          onClick={() => actionMutation.mutate('CheckNow')}
        >
          {t('updates.checkNow')}
        </Button>
        {status?.phase === 'PendingApproval' && (
          <>
            <Button
              type="button"
              variant="primary"
              size="sm"
              isLoading={actionMutation.isPending}
              onClick={() => actionMutation.mutate('Approve')}
              disabled={!canApproveOrDefer}
            >
              {t('updates.approve')}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              isLoading={actionMutation.isPending}
              onClick={() => actionMutation.mutate('Defer')}
              disabled={!canApproveOrDefer}
            >
              {t('updates.defer')}
            </Button>
          </>
        )}
        {status?.phase === 'ReadyToInstall' && (
          <>
            <Button
              type="button"
              variant="primary"
              size="sm"
              isLoading={actionMutation.isPending}
              onClick={() => actionMutation.mutate('Approve')}
              disabled={!canApproveOrDefer}
            >
              {t('updates.installNow', 'Install Now')}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              isLoading={actionMutation.isPending}
              onClick={() => actionMutation.mutate('Defer')}
              disabled={!canApproveOrDefer}
            >
              {t('updates.defer')}
            </Button>
          </>
        )}
      </div>

      {actionMutation.isError && (
        <div className={cn('mt-3 text-semantic-error', typography.caption)}>
          {(actionMutation.error as Error).message}
        </div>
      )}
    </Card>
  )
}
