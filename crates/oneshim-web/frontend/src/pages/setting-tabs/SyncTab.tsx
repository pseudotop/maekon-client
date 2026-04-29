import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '../../components/ui/Button'
import { GuidancePanel } from '../../components/ui/Guidance'
import { Spinner } from '../../components/ui/Spinner'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface SyncStatus {
  enabled: boolean
  device_id: string
  device_name: string
}

interface SyncResult {
  applied: number
  skipped: number
  tombstoned: number
}

interface SyncPeer {
  device_id: string
  device_name: string
  last_sync_at: string
}

const DISABLED_SYNC_STATUS: SyncStatus = {
  enabled: false,
  device_id: 'local-dev',
  device_name: 'Local device',
}

async function tauriInvoke<T>(cmd: string): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd)
}

export default function SyncTab() {
  const { t } = useTranslation()
  const [status, setStatus] = useState<SyncStatus | null>(null)
  const [peers, setPeers] = useState<SyncPeer[]>([])
  const [syncing, setSyncing] = useState(false)
  const [lastResult, setLastResult] = useState<SyncResult | null>(null)
  const [error, setError] = useState<string | null>(null)

  const fetchStatus = useCallback(async () => {
    try {
      const s = await tauriInvoke<SyncStatus>('get_sync_status')
      setStatus(s)
      if (s.enabled) {
        const p = await tauriInvoke<SyncPeer[]>('discover_sync_peers')
        setPeers(p)
      }
    } catch {
      setStatus(DISABLED_SYNC_STATUS)
      setPeers([])
      setError(null)
    }
  }, [])

  useEffect(() => {
    fetchStatus()
    // Poll every 30s to catch background sync completions
    const interval = setInterval(fetchStatus, 30_000)
    return () => clearInterval(interval)
  }, [fetchStatus])

  const handleSync = async () => {
    setSyncing(true)
    setError(null)
    try {
      const result = await tauriInvoke<SyncResult>('trigger_sync_cycle')
      setLastResult(result)
      await fetchStatus()
    } catch (e) {
      setError(String(e))
    } finally {
      setSyncing(false)
    }
  }

  if (!status) {
    return (
      <div className="flex items-center gap-2">
        <Spinner size="sm" />
        <span className={cn('text-sm', colors.text.tertiary)}>{t('syncTab.loading')}</span>
      </div>
    )
  }

  if (!status.enabled) {
    return (
      <div className="space-y-4">
        <h2 className={cn(typography.h2, colors.text.primary)}>{t('syncTab.title')}</h2>
        <GuidancePanel
          title={t('settings.guidance.sync.title')}
          description={t('settings.guidance.sync.description')}
          items={[
            {
              title: t('settings.guidance.sync.transport.title'),
              description: t('settings.guidance.sync.transport.description'),
            },
            {
              title: t('settings.guidance.sync.passphrase.title'),
              description: t('settings.guidance.sync.passphrase.description'),
            },
            {
              title: t('settings.guidance.sync.restart.title'),
              description: t('settings.guidance.sync.restart.description'),
            },
          ]}
        />
        <div className={cn('rounded-lg border p-4', colors.surface.muted)}>
          <p className={cn('text-sm', colors.text.secondary)}>{t('syncTab.notEnabled')}</p>
          <ol className={cn('mt-2 list-decimal space-y-1 pl-6 text-sm', colors.text.tertiary)}>
            <li>
              {t('syncTab.step1')} <code className={cn('rounded px-1', 'bg-surface-muted')}>sync.enabled = true</code>{' '}
              {t('syncTab.step1Suffix')}
            </li>
            <li>
              {t('syncTab.step2')}{' '}
              <code className={cn('rounded px-1', 'bg-surface-muted')}>ONESHIM_SYNC_PASSPHRASE</code>{' '}
              {t('syncTab.step2Suffix')}
            </li>
            <li>{t('syncTab.step3')}</li>
            <li>{t('syncTab.step4')}</li>
          </ol>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <h2 className={cn(typography.h2, colors.text.primary)}>{t('syncTab.title')}</h2>
      <GuidancePanel
        title={t('settings.guidance.sync.title')}
        description={t('settings.guidance.sync.enabledDescription')}
        items={[
          {
            title: t('settings.guidance.sync.device.title'),
            description: t('settings.guidance.sync.device.description'),
          },
          {
            title: t('settings.guidance.sync.peers.title'),
            description: t('settings.guidance.sync.peers.description'),
          },
          {
            title: t('settings.guidance.sync.conflicts.title'),
            description: t('settings.guidance.sync.conflicts.description'),
          },
        ]}
      />

      {/* Device Info */}
      <div className={cn('rounded-lg border p-4', colors.surface.elevated)}>
        <h3 className={cn('mb-2 text-sm', typography.weight.semibold, colors.text.primary)}>
          {t('syncTab.thisDevice')}
        </h3>
        <div className="grid grid-cols-2 gap-2 text-sm">
          <span className={colors.text.tertiary}>{t('syncTab.deviceName')}</span>
          <span className={colors.text.primary}>{status.device_name}</span>
          <span className={colors.text.tertiary}>{t('syncTab.deviceId')}</span>
          <span className={cn(typography.family.mono, 'text-xs', colors.text.secondary)}>
            {status.device_id.slice(0, 12)}...
          </span>
        </div>
      </div>

      {/* Sync Action */}
      <div className="flex items-center gap-3">
        <Button onClick={handleSync} disabled={syncing} isLoading={syncing} variant="primary" size="md">
          {syncing ? t('syncTab.syncing') : t('syncTab.syncNow')}
        </Button>
        {lastResult && (
          <span className={cn('text-sm', colors.text.tertiary)}>
            {t('syncTab.applied')}: {lastResult.applied}, {t('syncTab.skipped')}: {lastResult.skipped}
          </span>
        )}
      </div>

      {error && <div className={cn('rounded-md p-3 text-sm', 'bg-semantic-error/10 text-semantic-error')}>{error}</div>}

      {/* Peers */}
      <div>
        <h3 className={cn('mb-2 text-sm', typography.weight.semibold, colors.text.primary)}>
          {t('syncTab.discoveredPeers')} ({peers.length})
        </h3>
        {peers.length === 0 ? (
          <p className={cn('text-sm', colors.text.tertiary)}>{t('syncTab.noPeers')}</p>
        ) : (
          <div className="space-y-2">
            {peers.map((peer) => (
              <div
                key={peer.device_id}
                className={cn('flex items-center justify-between rounded-lg border p-3', colors.surface.elevated)}
              >
                <div>
                  <span className={cn('text-sm', typography.weight.medium, colors.text.primary)}>
                    {peer.device_name}
                  </span>
                  <span className={cn('ml-2 text-xs', typography.family.mono, colors.text.tertiary)}>
                    {peer.device_id.slice(0, 8)}
                  </span>
                </div>
                <span className={cn('text-xs', colors.text.tertiary)}>
                  {peer.last_sync_at
                    ? `${t('syncTab.lastSync')}: ${new Date(peer.last_sync_at).toLocaleString()}`
                    : t('syncTab.neverSynced')}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
