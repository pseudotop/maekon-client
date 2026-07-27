import { invoke } from '@tauri-apps/api/core'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { updateSyncSetup } from '../../api/syncSetup'
import { Button } from '../../components/ui/Button'
import { GuidancePanel } from '../../components/ui/Guidance'
import { Spinner } from '../../components/ui/Spinner'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface SyncStatus {
  enabled: boolean
  runtime_available: boolean
  runtime_state: 'ready' | 'disabled' | 'unavailable'
  unavailable_reason?: string | null
  device_id: string
  device_name: string
}

interface SyncResult {
  applied: number
  skipped: number
  tombstoned: number
  // #8056 P3: distinguishes "nothing to push" from "pushed but reached 0 peers".
  push_attempted?: boolean
  pushed_to_peers?: number
}

// #8056 P2-2: the runtime setup floor mirrors MIN_SYNC_PASSPHRASE_LEN (server-side).
const MIN_SYNC_PASSPHRASE_LEN = 12

interface SyncPeer {
  device_id: string
  device_name: string
  last_sync_at: string
}

interface UploadSpoolQcStatus {
  phase: 'interrupted' | 'recovered'
  pending_count: number
  attempt_count: number
  reprime_count: number
  sent_marker_count: number
  storage_id_preserved: boolean
  network_disabled: boolean
  real_account_used: boolean
  last_error?: string | null
}

const DISABLED_SYNC_STATUS: SyncStatus = {
  enabled: false,
  runtime_available: false,
  runtime_state: 'unavailable',
  unavailable_reason: 'tauri-unavailable',
  device_id: 'local-dev',
  device_name: 'Local device',
}

const tauriInvoke = <T,>(cmd: string, args?: Record<string, unknown>): Promise<T> => invoke<T>(cmd, args)

function UploadSpoolQcPanel() {
  const { t } = useTranslation()
  const [status, setStatus] = useState<UploadSpoolQcStatus | null>(null)
  const [running, setRunning] = useState(false)
  const [stepError, setStepError] = useState(false)

  useEffect(() => {
    let active = true
    tauriInvoke<UploadSpoolQcStatus | null>('get_qc_upload_spool_status')
      .then((next) => {
        if (active) setStatus(next)
      })
      .catch(() => {
        // The command intentionally returns no fixture in release builds and
        // ordinary profiles. Keep the QC-only surface completely hidden.
      })
    return () => {
      active = false
    }
  }, [])

  if (!status) return null

  const runStep = async () => {
    setRunning(true)
    setStepError(false)
    try {
      setStatus(await tauriInvoke<UploadSpoolQcStatus>('run_qc_upload_spool_step'))
    } catch {
      setStepError(true)
    } finally {
      setRunning(false)
    }
  }

  return (
    <section
      aria-label={t('syncTab.uploadRecoveryTitle')}
      className={cn('space-y-3 rounded-lg border p-4', colors.surface.elevated)}
    >
      <div className="space-y-1">
        <h3 className={cn('text-sm', typography.weight.semibold, colors.text.primary)}>
          {t('syncTab.uploadRecoveryTitle')}
        </h3>
        <p className={cn('text-sm', colors.text.secondary)}>{t('syncTab.uploadRecoveryDescription')}</p>
      </div>

      {status.phase === 'interrupted' && (
        <div className="space-y-2">
          <p className="text-semantic-warning text-sm" role="alert">
            {t('syncTab.uploadRecoveryInterrupted', { count: status.pending_count })}
          </p>
          <Button onClick={runStep} disabled={running} isLoading={running} variant="secondary" size="sm">
            {t('syncTab.uploadRecoveryRetry')}
          </Button>
        </div>
      )}

      {status.phase === 'recovered' && (
        <p className={cn('text-sm', colors.text.secondary)} role="status">
          {t('syncTab.uploadRecoveryRecovered', {
            attempts: status.attempt_count,
            reprimes: status.reprime_count,
          })}
        </p>
      )}

      <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
        <dt className={colors.text.tertiary}>{t('syncTab.uploadRecoveryPending')}</dt>
        <dd className={colors.text.secondary}>{status.pending_count}</dd>
        <dt className={colors.text.tertiary}>{t('syncTab.uploadRecoverySentMarkers')}</dt>
        <dd className={colors.text.secondary}>{status.sent_marker_count}</dd>
        <dt className={colors.text.tertiary}>{t('syncTab.uploadRecoveryStorageId')}</dt>
        <dd className={colors.text.secondary}>
          {status.storage_id_preserved ? t('syncTab.uploadRecoveryPreserved') : t('syncTab.uploadRecoveryMismatch')}
        </dd>
      </dl>

      {stepError && <p className="text-semantic-error text-sm">{t('syncTab.uploadRecoveryError')}</p>}
    </section>
  )
}

export default function SyncTab() {
  const { t } = useTranslation()
  const [status, setStatus] = useState<SyncStatus | null>(null)
  const [peers, setPeers] = useState<SyncPeer[]>([])
  const [syncing, setSyncing] = useState(false)
  const [lastResult, setLastResult] = useState<SyncResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  // #8056 P2-2: keychain-backed enable/passphrase setup form.
  const [passphrase, setPassphrase] = useState('')
  const [transport, setTransport] = useState<'lan' | 'file' | 'remote'>('lan')
  const [enabling, setEnabling] = useState(false)
  const [setupError, setSetupError] = useState<string | null>(null)
  const [setupDone, setSetupDone] = useState(false)
  const [pendingForgetPeer, setPendingForgetPeer] = useState<SyncPeer | null>(null)
  const [forgettingPeerId, setForgettingPeerId] = useState<string | null>(null)
  const [forgetError, setForgetError] = useState<string | null>(null)
  const [forgottenPeerName, setForgottenPeerName] = useState<string | null>(null)

  const fetchStatus = useCallback(async () => {
    try {
      const s = await tauriInvoke<SyncStatus>('get_sync_status')
      setStatus(s)
      if (s.runtime_state === 'ready') {
        const p = await tauriInvoke<SyncPeer[]>('discover_sync_peers')
        setPeers(p)
      } else {
        setPeers([])
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

  const handleForgetPeer = async () => {
    if (!pendingForgetPeer) return

    const peer = pendingForgetPeer
    setForgettingPeerId(peer.device_id)
    setForgetError(null)
    setForgottenPeerName(null)
    try {
      const remainingPeers = await tauriInvoke<SyncPeer[]>('forget_sync_peer', {
        deviceId: peer.device_id,
      })
      setPeers(remainingPeers)
      setPendingForgetPeer(null)
      setForgottenPeerName(peer.device_name)
    } catch {
      setForgetError(t('syncTab.forgetPeerError'))
    } finally {
      setForgettingPeerId(null)
    }
  }

  // #8056 P2-2: store the passphrase in the OS keychain + flip the enable switch
  // via the local REST API. Activation happens on the next app start.
  const handleEnableSync = async () => {
    setSetupError(null)
    if (passphrase.length < MIN_SYNC_PASSPHRASE_LEN) {
      setSetupError(
        t('syncTab.passphraseTooShort', {
          defaultValue: `Passphrase must be at least ${MIN_SYNC_PASSPHRASE_LEN} characters.`,
          count: MIN_SYNC_PASSPHRASE_LEN,
        }),
      )
      return
    }
    setEnabling(true)
    try {
      await updateSyncSetup({ passphrase, enabled: true, transport })
      setPassphrase('')
      setSetupDone(true)
      await fetchStatus()
    } catch (e) {
      setSetupError(e instanceof Error ? e.message : String(e))
    } finally {
      setEnabling(false)
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

  if (status.runtime_state === 'unavailable') {
    return (
      <div className="space-y-4">
        <h2 className={cn(typography.h2, colors.text.primary)}>{t('syncTab.title')}</h2>
        <UploadSpoolQcPanel />
        <GuidancePanel
          title={t('settings.guidance.sync.title')}
          description={t('settings.guidance.sync.description')}
          items={[
            {
              title: t('settings.guidance.sync.transport.title'),
              description: t('settings.guidance.sync.transport.description'),
            },
            {
              title: t('settings.guidance.sync.restart.title'),
              description: t('settings.guidance.sync.restart.description'),
            },
          ]}
        />
        <div className={cn('rounded-lg border p-4', colors.surface.muted)}>
          <p className={cn('text-sm', colors.text.secondary)}>{t('syncTab.unavailable')}</p>
          {status.unavailable_reason && (
            <p className={cn('mt-2 text-xs', colors.text.tertiary)}>{t('syncTab.unavailableHint')}</p>
          )}
        </div>
      </div>
    )
  }

  if (status.runtime_state === 'disabled' || !status.enabled) {
    return (
      <div className="space-y-4">
        <h2 className={cn(typography.h2, colors.text.primary)}>{t('syncTab.title')}</h2>
        <UploadSpoolQcPanel />
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
        <div className={cn('space-y-3 rounded-lg border p-4', colors.surface.muted)}>
          <p className={cn('text-sm', colors.text.secondary)}>
            {t('syncTab.enableIntro', {
              defaultValue:
                'Choose a passphrase and enable sync. It is stored in your OS keychain (never in the config file) and is used to derive the end-to-end encryption key. All devices you sync must use the same passphrase.',
            })}
          </p>
          <label className={cn('block text-sm', colors.text.tertiary)} htmlFor="sync-passphrase">
            {t('syncTab.passphraseLabel', { defaultValue: 'Sync passphrase' })}
            <input
              id="sync-passphrase"
              type="password"
              value={passphrase}
              onChange={(e) => setPassphrase(e.target.value)}
              placeholder={t('syncTab.passphrasePlaceholder', { defaultValue: 'At least 12 characters' })}
              autoComplete="new-password"
              className={cn(
                'mt-1 w-full rounded-md border px-3 py-2 text-sm',
                colors.surface.elevated,
                colors.text.primary,
              )}
            />
          </label>
          <label className={cn('block text-sm', colors.text.tertiary)} htmlFor="sync-transport">
            {t('syncTab.transportLabel', { defaultValue: 'Transport' })}
            <select
              id="sync-transport"
              value={transport}
              onChange={(e) => setTransport(e.target.value as 'lan' | 'file' | 'remote')}
              className={cn(
                'mt-1 w-full rounded-md border px-3 py-2 text-sm',
                colors.surface.elevated,
                colors.text.primary,
              )}
            >
              <option value="lan">{t('syncTab.transportLan', { defaultValue: 'LAN — same Wi-Fi/network' })}</option>
              <option value="file">
                {t('syncTab.transportFile', { defaultValue: 'Shared folder — Dropbox, iCloud, NAS' })}
              </option>
              <option value="remote">{t('syncTab.transportRemote', { defaultValue: 'Remote sync server' })}</option>
            </select>
          </label>
          <Button onClick={handleEnableSync} disabled={enabling} isLoading={enabling} variant="primary" size="md">
            {t('syncTab.enableButton', { defaultValue: 'Enable sync' })}
          </Button>
          {setupError && (
            <div className={cn('rounded-md p-2 text-sm', 'bg-semantic-error/10 text-semantic-error')}>{setupError}</div>
          )}
          {setupDone && (
            <div className={cn('rounded-md p-2 text-sm', 'bg-semantic-success/10 text-semantic-success')}>
              {t('syncTab.enableDone', { defaultValue: 'Saved. Restart the app to activate sync.' })}
            </div>
          )}
          <p className={cn('text-xs', colors.text.tertiary)}>
            {t('syncTab.envHint', {
              defaultValue:
                'Power users can instead set the MAEKON_SYNC_PASSPHRASE environment variable before launch.',
            })}
          </p>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <h2 className={cn(typography.h2, colors.text.primary)}>{t('syncTab.title')}</h2>
      <UploadSpoolQcPanel />
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

      {/* #8056 P3: surface a delivery warning when local changes reached no peer,
          so a "0 peers received" cycle is not mistaken for a clean no-op. */}
      {lastResult?.push_attempted && (lastResult.pushed_to_peers ?? 0) === 0 && (
        <div className={cn('rounded-md p-3 text-sm', 'bg-semantic-warning/10 text-semantic-warning')}>
          {t('syncTab.deliveryNoPeers', {
            defaultValue:
              'Your changes could not reach any peer (all offline or unreachable). They will retry on the next sync.',
          })}
        </div>
      )}
      {lastResult?.push_attempted && (lastResult.pushed_to_peers ?? 0) > 0 && (
        <span className={cn('text-xs', colors.text.tertiary)}>
          {t('syncTab.deliveredToPeers', {
            defaultValue: `Delivered to ${lastResult.pushed_to_peers} peer(s).`,
            count: lastResult.pushed_to_peers,
          })}
        </span>
      )}

      {error && (
        <div className={cn('space-y-2 rounded-md p-3 text-sm', 'bg-semantic-error/10 text-semantic-error')}>
          <p>{error}</p>
          {/* #8079 (CRT-PRV-UX-RECOVERY-001): a failed sync cycle now offers a
              retry instead of a dead-end error string. */}
          <Button onClick={handleSync} disabled={syncing} isLoading={syncing} variant="secondary" size="sm">
            {t('common.retry')}
          </Button>
        </div>
      )}

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
              <div key={peer.device_id} className="space-y-2">
                <div
                  className={cn(
                    'flex items-center justify-between gap-3 rounded-lg border p-3',
                    colors.surface.elevated,
                  )}
                >
                  <div>
                    <span className={cn('text-sm', typography.weight.medium, colors.text.primary)}>
                      {peer.device_name}
                    </span>
                    <span className={cn('ml-2 text-xs', typography.family.mono, colors.text.tertiary)}>
                      {peer.device_id.slice(0, 8)}
                    </span>
                  </div>
                  <div className="flex items-center gap-2">
                    <span className={cn('text-xs', colors.text.tertiary)}>
                      {peer.last_sync_at
                        ? `${t('syncTab.lastSync')}: ${new Date(peer.last_sync_at).toLocaleString()}`
                        : t('syncTab.neverSynced')}
                    </span>
                    <Button
                      onClick={() => {
                        setPendingForgetPeer(peer)
                        setForgetError(null)
                        setForgottenPeerName(null)
                      }}
                      variant="ghost"
                      size="sm"
                    >
                      {t('syncTab.forgetPeer')}
                    </Button>
                  </div>
                </div>
                {pendingForgetPeer?.device_id === peer.device_id && (
                  <div
                    role="alertdialog"
                    aria-labelledby={`forget-peer-title-${peer.device_id}`}
                    className={cn('rounded-lg border p-3', colors.surface.muted)}
                  >
                    <p id={`forget-peer-title-${peer.device_id}`} className={cn('text-sm', colors.text.primary)}>
                      {t('syncTab.forgetPeerConfirm', { peerName: peer.device_name })}
                    </p>
                    <div className="mt-3 flex gap-2">
                      <Button
                        onClick={handleForgetPeer}
                        disabled={forgettingPeerId === peer.device_id}
                        isLoading={forgettingPeerId === peer.device_id}
                        variant="danger"
                        size="sm"
                      >
                        {t('syncTab.forgetPeerConfirmAction')}
                      </Button>
                      <Button
                        onClick={() => setPendingForgetPeer(null)}
                        disabled={forgettingPeerId === peer.device_id}
                        variant="secondary"
                        size="sm"
                      >
                        {t('syncTab.forgetPeerCancel')}
                      </Button>
                    </div>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
        {forgottenPeerName && (
          <p role="status" aria-live="polite" className={cn('mt-3 text-sm', colors.text.secondary)}>
            {t('syncTab.forgetPeerSuccess', { peerName: forgottenPeerName })}
          </p>
        )}
        {forgetError && (
          <p role="alert" className="mt-3 text-semantic-error text-sm">
            {forgetError}
          </p>
        )}
      </div>
    </div>
  )
}
