/**
 * ADR-033 memory vault mirror settings section (#9465, PrivacyTab).
 *
 * Self-contained — managed via dedicated IPC, separate from the config form
 * (the same shape as `CaptureReauthSettings`):
 * - Enable: Tier-13 `memory_vault_mirror` consent (`set_consent`) AND
 *   `analysis.memory_vault.enabled` (`update_setting`). The writer requires
 *   BOTH (ADR-033 §2.3), so one user-facing switch sets both and the rendered
 *   state is the AND of them — never config alone, which would show "on" while
 *   every cycle fail-closed no-ops.
 * - Custom path: `set_vault_mirror_path`, a two-step flow. Step 1 proposes the
 *   path and gets back the §3.2 cloud-sync detection WITHOUT writing config;
 *   step 2 re-submits with the §3.3 acknowledgement. Every custom path needs
 *   the acknowledgement, detected or not — an unlisted sync tool must not
 *   produce a silent "no warning at all" path.
 * - "Export now": `run_vault_mirror_cycle` — a FULL cycle (§7.5), not a
 *   today-only export.
 *
 * The copy is deliberately blunt about the one-way contract: generated files
 * are overwritten, and Maekon never reads their content back.
 */

import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { getConsent, setConsent } from '../../api/client'
import type { ConsentPermissions, ConsentSnapshot } from '../../api/contracts'
import {
  acceptVaultMirrorPath,
  clearVaultMirrorPath,
  getVaultMirrorSettings,
  previewVaultMirrorPath,
  runVaultMirrorCycle,
  setVaultMirrorEnabled,
  type VaultMirrorSettings as VaultSettings,
} from '../../api/vault'
import { Alert, Button, Card, CardTitle, Checkbox, Input } from '../../components/ui'
import { useLatestOnlyRead } from '../../hooks/useLatestOnlyRead'
import { addToast } from '../../hooks/useToast'
import { describeIpcError } from '../../i18n/tauriIpcErrors'
import { typography } from '../../styles/tokens'
import { formatDateTime } from '../../utils/formatters'
import ToggleRow from './ToggleRow'

/** Stable id so the enable Checkbox can point at the one-way disclosure. */
const ONE_WAY_DISCLOSURE_ID = 'memory-vault-one-way-disclosure'

/** A custom path proposed but not yet acknowledged (ADR-033 §3.3 step 1). */
interface PendingPath {
  readonly path: string
  /** §3.2 detection: `null` = nothing recognized, which still warrants a warning. */
  readonly cloudProvider: string | null
}

/** Coarse provider labels that have their own localized name. */
const PROVIDER_KEYS: Readonly<Record<string, string>> = {
  icloud: 'settings.memoryVault.provider.icloud',
  cloud_storage: 'settings.memoryVault.provider.cloudStorage',
  onedrive: 'settings.memoryVault.provider.onedrive',
  dropbox: 'settings.memoryVault.provider.dropbox',
  google_drive: 'settings.memoryVault.provider.googleDrive',
}

export default function MemoryVaultSettings() {
  const { t, i18n } = useTranslation()
  const [settings, setSettings] = useState<VaultSettings | null>(null)
  const [consent, setConsentSnapshot] = useState<ConsentSnapshot | null>(null)
  const [busy, setBusy] = useState(false)
  const [pathInput, setPathInput] = useState('')
  const [pending, setPending] = useState<PendingPath | null>(null)
  const [acknowledged, setAcknowledged] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const read = useLatestOnlyRead()

  const refresh = useCallback(async () => {
    const token = read.begin()
    try {
      const [next, snapshot] = await Promise.all([getVaultMirrorSettings(), getConsent()])
      if (read.isCurrent(token)) {
        setSettings(next)
        setConsentSnapshot(snapshot)
      }
    } catch {
      // Tauri unavailable (standalone/demo build) — hide the section entirely
      // rather than showing a control that cannot work.
      if (read.isCurrent(token)) setSettings(null)
    }
  }, [read])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const describe = useCallback((err: unknown) => describeIpcError(err, t, i18n.language), [i18n.language, t])

  /**
   * Tier-13 grant + `enabled`, in the order that fails safe at every step:
   * enabling grants consent first (config-on without consent still no-ops),
   * disabling turns config off first (so the very next cycle stops even if the
   * consent write then fails).
   */
  const applyEnabled = useCallback(
    async (next: boolean) => {
      if (!consent) return
      setBusy(true)
      setError(null)
      try {
        const permissions: ConsentPermissions = {
          ...consent.permissions,
          memory_vault_mirror: next,
        }
        if (next) {
          await setConsent(permissions)
          await setVaultMirrorEnabled(true)
        } else {
          await setVaultMirrorEnabled(false)
          await setConsent(permissions)
        }
        await refresh()
      } catch (err) {
        setError(describe(err))
      } finally {
        setBusy(false)
      }
    },
    [consent, describe, refresh],
  )

  /** §3.3 step 1: detect, do not write. */
  const proposePath = useCallback(async () => {
    setBusy(true)
    setError(null)
    try {
      const outcome = await previewVaultMirrorPath(pathInput)
      setPending({ path: pathInput.trim(), cloudProvider: outcome.cloud_provider })
      setAcknowledged(false)
    } catch (err) {
      setError(describe(err))
    } finally {
      setBusy(false)
    }
  }, [describe, pathInput])

  /**
   * §3.3 step 2: the acknowledgement is what unlocks the write.
   *
   * The `!acknowledged` half of this guard is deliberate defence-in-depth and is
   * NOT independently observable: the confirm control is `disabled` until the
   * box is ticked, so jsdom never dispatches a click that reaches this branch.
   * Mutating the guard away leaves the suite green while mutating the
   * `disabled` prop away turns it red — the *behavior* is covered, this belt is
   * review-guarded (the same residual `commands/settings.rs` documents for its
   * forbidden-key helper call site). It exists because `disabled` is a
   * rendering concern a later refactor to a different control could drop
   * silently, whereas this line cannot be dropped without deleting it.
   *
   * The backend enforces the same gate independently: `set_vault_mirror_path`
   * writes nothing unless `acknowledged` is true.
   */
  const confirmPath = useCallback(async () => {
    if (!pending || !acknowledged) return
    setBusy(true)
    setError(null)
    try {
      await acceptVaultMirrorPath(pending.path)
      setPending(null)
      setAcknowledged(false)
      setPathInput('')
      addToast('success', t('settings.memoryVault.pathSaved'))
      await refresh()
    } catch (err) {
      setError(describe(err))
    } finally {
      setBusy(false)
    }
  }, [acknowledged, describe, pending, refresh, t])

  const revertToDefault = useCallback(async () => {
    setBusy(true)
    setError(null)
    try {
      await clearVaultMirrorPath()
      setPending(null)
      setAcknowledged(false)
      setPathInput('')
      addToast('success', t('settings.memoryVault.pathCleared'))
      await refresh()
    } catch (err) {
      setError(describe(err))
    } finally {
      setBusy(false)
    }
  }, [describe, refresh, t])

  const exportNow = useCallback(async () => {
    setBusy(true)
    setError(null)
    try {
      const stats = await runVaultMirrorCycle()
      if (stats.skipped_reason) {
        // A fail-closed skip is a SUCCESSFUL call with zero counters. Reporting
        // it as "exported" is the lie this branch exists to prevent.
        addToast('warning', t('settings.memoryVault.exportSkipped', { reason: stats.skipped_reason }))
      } else {
        addToast(
          'success',
          t('settings.memoryVault.exportDone', {
            days: stats.day_files_written,
            expired: stats.files_expired,
          }),
        )
      }
      if (stats.conflicts > 0) {
        addToast('warning', t('settings.memoryVault.conflictsNotice', { count: stats.conflicts }))
      }
      await refresh()
    } catch (err) {
      setError(describe(err))
      addToast('error', t('settings.memoryVault.exportFailed'))
    } finally {
      setBusy(false)
    }
  }, [describe, refresh, t])

  if (settings === null) return null

  // Persisted §6.4 status (#9522). `null` means no cycle has run — rendered as
  // exactly that, never as a zero-count cycle that would claim one had.
  const lastCycle = settings.last_cycle
  const consentGranted = consent?.status === 'Valid' && consent.permissions.memory_vault_mirror === true
  // The writer needs config AND consent (§2.3); showing either alone as "on"
  // would misreport a permanently no-op mirror as running.
  const mirrorOn = settings.enabled && consentGranted
  const providerName = (label: string): string => {
    const key = PROVIDER_KEYS[label]
    return key ? t(key) : label
  }

  return (
    <Card variant="default" padding="lg" id="section-memory-vault">
      <CardTitle>{t('settings.memoryVault.sectionTitle')}</CardTitle>
      <p className="mb-4 text-content-secondary text-sm">{t('settings.memoryVault.sectionDescription')}</p>

      <Alert id={ONE_WAY_DISCLOSURE_ID} variant="info" className="mb-4">
        {t('settings.memoryVault.oneWayNotice')}
      </Alert>

      {error && (
        <Alert variant="error" className="mb-4">
          {error}
        </Alert>
      )}

      <div className="space-y-4">
        <ToggleRow
          label={t('settings.memoryVault.enableLabel')}
          description={t('settings.memoryVault.enableHint')}
          checked={mirrorOn}
          disabled={busy || consent === null}
          describedBy={ONE_WAY_DISCLOSURE_ID}
          onChange={(checked) => void applyEnabled(checked)}
        />

        {settings.enabled && !consentGranted && (
          <Alert variant="warning">{t('settings.memoryVault.consentPending')}</Alert>
        )}

        {!settings.window_within_bound && (
          <Alert variant="warning">
            {t('settings.memoryVault.windowOutOfBound', { days: settings.mirror_window_days })}
          </Alert>
        )}

        <div className="rounded-md border border-border p-3">
          <p className={typography.label}>{t('settings.memoryVault.activePathLabel')}</p>
          <p className="mt-1 break-all text-content-secondary text-xs" data-testid="vault-active-path">
            {settings.active_path ?? t('settings.memoryVault.pathUnresolved')}
          </p>
          {settings.custom_path === null && (
            <p className="mt-1 text-content-tertiary text-xs">{t('settings.memoryVault.defaultPathNote')}</p>
          )}
          {settings.custom_path !== null && !settings.custom_path_acknowledged && (
            <Alert variant="warning" className="mt-2">
              {t('settings.memoryVault.pendingPathNote', { path: settings.custom_path })}
            </Alert>
          )}
          {settings.cloud_provider !== null && settings.custom_path_acknowledged && (
            <Alert variant="warning" className="mt-2" data-testid="vault-cloud-active-notice">
              {t('settings.memoryVault.cloudActiveNotice', { provider: providerName(settings.cloud_provider) })}
            </Alert>
          )}

          <div className="mt-3 space-y-2">
            <Input
              aria-label={t('settings.memoryVault.pathInputLabel')}
              placeholder={t('settings.memoryVault.pathInputPlaceholder')}
              value={pathInput}
              disabled={busy}
              onChange={(e) => setPathInput(e.target.value)}
            />
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                variant="secondary"
                data-testid="vault-propose-path"
                disabled={busy || pathInput.trim().length === 0}
                onClick={() => void proposePath()}
              >
                {t('settings.memoryVault.choosePath')}
              </Button>
              {settings.custom_path !== null && (
                <Button
                  type="button"
                  variant="ghost"
                  data-testid="vault-use-default"
                  disabled={busy}
                  onClick={() => void revertToDefault()}
                >
                  {t('settings.memoryVault.useDefault')}
                </Button>
              )}
            </div>
          </div>

          {/* §3.3: the acknowledgement gate. Both risks are stated before the
              confirm control becomes usable, and the sync sentence names the
              provider only when detection actually recognized one. */}
          {pending && (
            <div
              className="mt-3 space-y-2 rounded-md border border-semantic-warning/30 p-3"
              data-testid="vault-path-warning"
            >
              <p className={typography.label}>{t('settings.memoryVault.warningTitle', { path: pending.path })}</p>
              <p className="text-content-secondary text-sm">{t('settings.memoryVault.warningOverwrite')}</p>
              <p className="text-content-secondary text-sm">
                {pending.cloudProvider
                  ? t('settings.memoryVault.warningSyncDetected', {
                      provider: providerName(pending.cloudProvider),
                    })
                  : t('settings.memoryVault.warningSyncUnknown')}
              </p>
              <label className="flex items-start gap-2 text-content-strong text-sm">
                <Checkbox
                  data-testid="vault-acknowledge"
                  checked={acknowledged}
                  disabled={busy}
                  onChange={(e) => setAcknowledged(e.target.checked)}
                />
                <span>{t('settings.memoryVault.warningAcknowledge')}</span>
              </label>
              <div className="flex gap-2">
                <Button
                  type="button"
                  variant="primary"
                  data-testid="vault-confirm-path"
                  isLoading={busy}
                  disabled={busy || !acknowledged}
                  onClick={() => void confirmPath()}
                >
                  {t('settings.memoryVault.warningConfirm')}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => {
                    setPending(null)
                    setAcknowledged(false)
                  }}
                >
                  {t('settings.memoryVault.warningCancel')}
                </Button>
              </div>
            </div>
          )}
        </div>

        <div className="rounded-md border border-border p-3">
          <p className={typography.label}>{t('settings.memoryVault.exportNow')}</p>
          <p className="mt-1 mb-2 text-content-secondary text-xs">{t('settings.memoryVault.exportNowHint')}</p>
          <Button
            type="button"
            variant="secondary"
            data-testid="vault-export-now"
            isLoading={busy}
            disabled={busy}
            onClick={() => void exportNow()}
          >
            {t('settings.memoryVault.exportNow')}
          </Button>
        </div>

        {/* §6.4: the persisted last cycle — the ONLY place a scheduled cycle's
            marker conflicts ever become visible. A cycle the user did not
            trigger reports its skipped files here on the next visit, instead of
            staying invisible until they happen to press "Export now". */}
        <div className="rounded-md border border-border p-3" data-testid="vault-last-cycle">
          <p className={typography.label}>{t('settings.memoryVault.lastCycleLabel')}</p>
          {lastCycle === null ? (
            <p className="mt-1 text-content-secondary text-xs">{t('settings.memoryVault.lastCycleNever')}</p>
          ) : (
            <>
              <p className="mt-1 text-content-secondary text-xs" data-testid="vault-last-cycle-summary">
                {t('settings.memoryVault.lastCycleSummary', {
                  time: formatDateTime(new Date(lastCycle.finished_at * 1000).toISOString(), i18n.language),
                  days: lastCycle.day_files_written,
                  expired: lastCycle.files_expired,
                })}
              </p>
              {lastCycle.conflicts > 0 && (
                <Alert variant="warning" className="mt-2" data-testid="vault-last-cycle-conflicts">
                  <p>{t('settings.memoryVault.conflictsTitle', { count: lastCycle.conflicts })}</p>
                  <p className="mt-1">{t('settings.memoryVault.conflictsExplain')}</p>
                  <ul className="mt-1 list-disc break-all pl-4">
                    {lastCycle.conflict_paths.map((path) => (
                      <li key={path}>{path}</li>
                    ))}
                  </ul>
                  {lastCycle.conflicts > lastCycle.conflict_paths.length && (
                    <p className="mt-1">
                      {t('settings.memoryVault.conflictsMore', {
                        count: lastCycle.conflicts - lastCycle.conflict_paths.length,
                      })}
                    </p>
                  )}
                </Alert>
              )}
            </>
          )}
        </div>
      </div>
    </Card>
  )
}
