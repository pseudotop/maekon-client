/**
 * ADR-033 (#9465) MemoryVaultSettings behavior test.
 *
 * Asserts the gates, not the copy: which IPC command is invoked with which
 * arguments, and — for the §3.3 acknowledgement — that config is NOT written
 * until the user has ticked it. Assertions reference resolved en.json copy
 * (test i18n falls back to en) so a wording change stays green while a key
 * rename goes red.
 */

import { fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import type { ConsentPermissions, ConsentSnapshot, ConsentStatus } from '../../api/contracts'
import type { VaultLastCycleSummary } from '../../api/vault'
import * as toastModule from '../../hooks/useToast'
import en from '../../i18n/locales/en.json'
import wireErrors from '../../i18n/wire-errors.en.json'
import MemoryVaultSettings from './MemoryVaultSettings'

// `api/vault` and `api/client` both dynamically import @tauri-apps/api/core, so
// intercept the module registry AND stub the internals bridge (the concurrent
// dynamic imports can otherwise race past vi.mock into jsdom, where
// window.__TAURI_INTERNALS__ is undefined) — the GeneralTab.test.tsx strategy.
const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

const ALL_FALSE: ConsentPermissions = {
  screen_capture: false,
  ocr_processing: false,
  telemetry: false,
  process_monitoring: false,
  input_activity: false,
  window_title_collection: false,
  app_usage_analytics: false,
  clipboard_monitoring: false,
  file_access_monitoring: false,
  activity_pattern_learning: false,
  cross_device_sync: false,
  full_text_extraction: false,
  memory_graph_enrichment: false,
  microphone: false,
  unredacted_external_ocr: false,
  memory_graph_retrieval_ranking: false,
  memory_vault_mirror: false,
}

const DEFAULT_PATH = '/Users/tester/Library/Application Support/Maekon/data/vault'

interface VaultSettingsFixture {
  enabled?: boolean
  active_path?: string | null
  default_path?: string | null
  custom_path?: string | null
  custom_path_acknowledged?: boolean
  cloud_provider?: string | null
  mirror_window_days?: number
  window_within_bound?: boolean
  /** #9522: the persisted §6.4 last-cycle summary; `null` = none has run. */
  last_cycle?: VaultLastCycleSummary | null
}

function vaultSettings(overrides: VaultSettingsFixture = {}) {
  return {
    enabled: false,
    active_path: DEFAULT_PATH,
    default_path: DEFAULT_PATH,
    custom_path: null,
    custom_path_acknowledged: false,
    cloud_provider: null,
    mirror_window_days: 90,
    window_within_bound: true,
    last_cycle: null,
    ...overrides,
  }
}

/** A recorded cycle, as `get_vault_mirror_settings` returns it. */
function lastCycle(overrides: Partial<VaultLastCycleSummary> = {}): VaultLastCycleSummary {
  return {
    finished_at: 1_753_000_000,
    day_files_written: 3,
    files_expired: 1,
    conflicts: 0,
    conflict_paths: [],
    ...overrides,
  }
}

function consentSnapshot(
  status: ConsentStatus = 'Valid',
  overrides: Partial<ConsentPermissions> = {},
): ConsentSnapshot {
  return { status, permissions: { ...ALL_FALSE, ...overrides } }
}

interface Handlers {
  settings?: VaultSettingsFixture
  consent?: ConsentSnapshot
  /** §3.2 detection result returned by `set_vault_mirror_path`. */
  cloudProvider?: string | null
  /** Force a specific command to reject. */
  failWith?: { cmd: string; error: unknown }
  cycle?: Record<string, unknown>
}

/** Records every invoke so tests can assert on arguments, not just calls. */
function mockVaultIpc(handlers: Handlers = {}) {
  const calls: { cmd: string; args: Record<string, unknown> | undefined }[] = []
  const current = vaultSettings(handlers.settings)
  mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    calls.push({ cmd, args })
    if (handlers.failWith && handlers.failWith.cmd === cmd) {
      return Promise.reject(handlers.failWith.error)
    }
    if (cmd === 'get_vault_mirror_settings') return Promise.resolve(current)
    if (cmd === 'get_consent') return Promise.resolve(handlers.consent ?? consentSnapshot())
    if (cmd === 'set_consent') {
      const permissions = (args as { permissions: ConsentPermissions }).permissions
      return Promise.resolve({ status: 'Valid', permissions } satisfies ConsentSnapshot)
    }
    if (cmd === 'update_setting') return Promise.resolve(undefined)
    if (cmd === 'set_vault_mirror_path') {
      const acknowledged = (args as { acknowledged: boolean }).acknowledged
      return Promise.resolve({
        cloud_provider: handlers.cloudProvider ?? null,
        applied: acknowledged,
        settings: current,
      })
    }
    if (cmd === 'run_vault_mirror_cycle') {
      return Promise.resolve({
        skipped_reason: null,
        day_files_written: 3,
        claims_file_written: true,
        files_expired: 1,
        conflicts: 0,
        conflict_paths: [],
        bytes_written: 2048,
        cloud_ledger_recorded: false,
        ...handlers.cycle,
      })
    }
    return Promise.resolve(undefined)
  })
  return {
    calls,
    of: (cmd: string) => calls.filter((c) => c.cmd === cmd),
  }
}

const copy = en.settings.memoryVault

/**
 * `ToggleRow` wraps the checkbox in a `<label>` whose text content is the label
 * AND the description, so the computed accessible name is the concatenation.
 * Match the label as a substring rather than pinning that concatenation.
 */
const ENABLE_TOGGLE = new RegExp(copy.enableLabel.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))

beforeEach(() => {
  mockInvoke.mockReset()
  ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: (...args: unknown[]) => mockInvoke(...args),
  }
})

describe('MemoryVaultSettings — enable gate', () => {
  it('renders the mirror as OFF when Tier-13 consent is not granted', async () => {
    // Fail-closed default: consent false AND enabled false (ADR-033 §2).
    mockVaultIpc()
    renderWithProviders(<MemoryVaultSettings />)

    const toggle = await screen.findByRole('checkbox', { name: ENABLE_TOGGLE })
    expect(toggle).not.toBeChecked()
    expect(screen.getByText(copy.oneWayNotice)).toBeInTheDocument()
  })

  it('associates the one-way disclosure with the enable toggle via aria-describedby', async () => {
    // A static disclosure sibling is not reliably announced (WAI-ARIA), so the
    // toggle must point at it. Asserted because the previous version only
    // *claimed* this in a comment while `ToggleRow` had no such prop at all.
    mockVaultIpc()
    renderWithProviders(<MemoryVaultSettings />)

    const toggle = await screen.findByRole('checkbox', { name: ENABLE_TOGGLE })
    const describedBy = toggle.getAttribute('aria-describedby')
    expect(describedBy).toBe('memory-vault-one-way-disclosure')
    const disclosure = document.getElementById(describedBy as string)
    expect(disclosure).not.toBeNull()
    expect(disclosure).toHaveTextContent(copy.oneWayNotice)
  })

  it('reports the mirror as OFF when config is enabled but Tier-13 consent is missing', async () => {
    // The writer needs BOTH (§2.3). Config alone must never render as "on",
    // or the UI claims a mirror is running while every cycle no-ops.
    mockVaultIpc({ settings: { enabled: true }, consent: consentSnapshot('Valid') })
    renderWithProviders(<MemoryVaultSettings />)

    const toggle = await screen.findByRole('checkbox', { name: ENABLE_TOGGLE })
    expect(toggle).not.toBeChecked()
    expect(screen.getByText(copy.consentPending)).toBeInTheDocument()
  })

  it('reports the mirror as OFF when consent is granted but the record is Expired', async () => {
    mockVaultIpc({
      settings: { enabled: true },
      consent: consentSnapshot('Expired', { memory_vault_mirror: true }),
    })
    renderWithProviders(<MemoryVaultSettings />)

    const toggle = await screen.findByRole('checkbox', { name: ENABLE_TOGGLE })
    expect(toggle).not.toBeChecked()
  })

  it('renders the mirror as ON only when consent is Valid AND config is enabled', async () => {
    mockVaultIpc({
      settings: { enabled: true },
      consent: consentSnapshot('Valid', { memory_vault_mirror: true }),
    })
    renderWithProviders(<MemoryVaultSettings />)

    const toggle = await screen.findByRole('checkbox', { name: ENABLE_TOGGLE })
    expect(toggle).toBeChecked()
    expect(screen.queryByText(copy.consentPending)).not.toBeInTheDocument()
  })

  it('enabling grants Tier-13 consent and sets the config flag, preserving other opt-ins', async () => {
    const ipc = mockVaultIpc({
      consent: consentSnapshot('Valid', { screen_capture: true, microphone: true }),
    })
    renderWithProviders(<MemoryVaultSettings />)

    const toggle = await screen.findByRole('checkbox', { name: ENABLE_TOGGLE })
    fireEvent.click(toggle)

    await waitFor(() => expect(ipc.of('set_consent')).toHaveLength(1))
    const sent = (ipc.of('set_consent')[0].args as { permissions: ConsentPermissions }).permissions
    expect(sent.memory_vault_mirror).toBe(true)
    // set_consent replaces the record wholesale — unrelated opt-ins must survive.
    expect(sent.screen_capture).toBe(true)
    expect(sent.microphone).toBe(true)

    await waitFor(() => expect(ipc.of('update_setting')).toHaveLength(1))
    const patch = JSON.parse((ipc.of('update_setting')[0].args as { configJson: string }).configJson)
    expect(patch).toEqual({ analysis: { memory_vault: { enabled: true } } })
  })

  it('disabling withdraws the Tier-13 grant and clears the config flag', async () => {
    const ipc = mockVaultIpc({
      settings: { enabled: true },
      consent: consentSnapshot('Valid', { memory_vault_mirror: true, screen_capture: true }),
    })
    renderWithProviders(<MemoryVaultSettings />)

    const toggle = await screen.findByRole('checkbox', { name: ENABLE_TOGGLE })
    fireEvent.click(toggle)

    await waitFor(() => expect(ipc.of('set_consent')).toHaveLength(1))
    const sent = (ipc.of('set_consent')[0].args as { permissions: ConsentPermissions }).permissions
    expect(sent.memory_vault_mirror).toBe(false)
    expect(sent.screen_capture).toBe(true)
    const patch = JSON.parse((ipc.of('update_setting')[0].args as { configJson: string }).configJson)
    expect(patch.analysis.memory_vault.enabled).toBe(false)
  })
})

describe('MemoryVaultSettings — §3.3 custom-path acknowledgement', () => {
  it('shows the default folder and no custom-path notice when none is configured', async () => {
    mockVaultIpc()
    renderWithProviders(<MemoryVaultSettings />)

    await waitFor(() => expect(screen.getByTestId('vault-active-path')).toHaveTextContent(DEFAULT_PATH))
    expect(screen.getByText(copy.defaultPathNote)).toBeInTheDocument()
    expect(screen.queryByTestId('vault-path-warning')).not.toBeInTheDocument()
  })

  it('proposing a path detects cloud sync WITHOUT writing config', async () => {
    const ipc = mockVaultIpc({ cloudProvider: 'icloud' })
    renderWithProviders(<MemoryVaultSettings />)

    const input = await screen.findByLabelText(copy.pathInputLabel)
    fireEvent.change(input, { target: { value: '/Users/tester/iCloud/vault' } })
    fireEvent.click(screen.getByTestId('vault-propose-path'))

    await waitFor(() => expect(screen.getByTestId('vault-path-warning')).toBeInTheDocument())
    const proposals = ipc.of('set_vault_mirror_path')
    expect(proposals).toHaveLength(1)
    // The write gate: the probe carries acknowledged=false, so the backend
    // returns the detection and leaves config untouched.
    expect(proposals[0].args).toEqual({
      path: '/Users/tester/iCloud/vault',
      acknowledged: false,
    })
  })

  it('names the detected provider in the sync warning', async () => {
    mockVaultIpc({ cloudProvider: 'dropbox' })
    renderWithProviders(<MemoryVaultSettings />)

    const input = await screen.findByLabelText(copy.pathInputLabel)
    fireEvent.change(input, { target: { value: '/Users/tester/Dropbox/vault' } })
    fireEvent.click(screen.getByTestId('vault-propose-path'))

    await waitFor(() => expect(screen.getByTestId('vault-path-warning')).toBeInTheDocument())
    expect(screen.getByText(copy.warningOverwrite)).toBeInTheDocument()
    expect(
      screen.getByText(copy.warningSyncDetected.replace('{{provider}}', copy.provider.dropbox)),
    ).toBeInTheDocument()
    expect(screen.queryByText(copy.warningSyncUnknown)).not.toBeInTheDocument()
  })

  it('still warns about sync when NO provider is detected', async () => {
    // §3.3: the detected/undetected split was deliberately removed — an
    // unlisted sync tool must not produce a path with no warning at all.
    mockVaultIpc({ cloudProvider: null })
    renderWithProviders(<MemoryVaultSettings />)

    const input = await screen.findByLabelText(copy.pathInputLabel)
    fireEvent.change(input, { target: { value: '/srv/notes/vault' } })
    fireEvent.click(screen.getByTestId('vault-propose-path'))

    await waitFor(() => expect(screen.getByTestId('vault-path-warning')).toBeInTheDocument())
    expect(screen.getByText(copy.warningOverwrite)).toBeInTheDocument()
    expect(screen.getByText(copy.warningSyncUnknown)).toBeInTheDocument()
  })

  it('the confirm control stays disabled until the acknowledgement is ticked', async () => {
    const ipc = mockVaultIpc({ cloudProvider: 'icloud' })
    renderWithProviders(<MemoryVaultSettings />)

    const input = await screen.findByLabelText(copy.pathInputLabel)
    fireEvent.change(input, { target: { value: '/Users/tester/iCloud/vault' } })
    fireEvent.click(screen.getByTestId('vault-propose-path'))
    await waitFor(() => expect(screen.getByTestId('vault-path-warning')).toBeInTheDocument())

    const confirm = screen.getByTestId('vault-confirm-path')
    expect(confirm).toBeDisabled()
    // Clicking through the disabled control must not reach the backend.
    fireEvent.click(confirm)
    expect(ipc.of('set_vault_mirror_path').filter((c) => c.args?.acknowledged === true)).toHaveLength(0)

    fireEvent.click(screen.getByTestId('vault-acknowledge'))
    await waitFor(() => expect(screen.getByTestId('vault-confirm-path')).toBeEnabled())
  })

  it('confirming after acknowledgement commits the path with acknowledged=true', async () => {
    const toastSpy = vi.spyOn(toastModule, 'addToast')
    const ipc = mockVaultIpc({ cloudProvider: 'icloud' })
    renderWithProviders(<MemoryVaultSettings />)

    const input = await screen.findByLabelText(copy.pathInputLabel)
    fireEvent.change(input, { target: { value: '/Users/tester/iCloud/vault' } })
    fireEvent.click(screen.getByTestId('vault-propose-path'))
    await waitFor(() => expect(screen.getByTestId('vault-path-warning')).toBeInTheDocument())
    fireEvent.click(screen.getByTestId('vault-acknowledge'))
    await waitFor(() => expect(screen.getByTestId('vault-confirm-path')).toBeEnabled())
    fireEvent.click(screen.getByTestId('vault-confirm-path'))

    await waitFor(() => {
      const commits = ipc.of('set_vault_mirror_path').filter((c) => c.args?.acknowledged === true)
      expect(commits).toHaveLength(1)
      expect(commits[0].args).toEqual({
        path: '/Users/tester/iCloud/vault',
        acknowledged: true,
      })
    })
    expect(toastSpy).toHaveBeenCalledWith('success', copy.pathSaved)
    // The warning panel closes only after the commit succeeds.
    await waitFor(() => expect(screen.queryByTestId('vault-path-warning')).not.toBeInTheDocument())
    toastSpy.mockRestore()
  })

  it('cancelling the warning writes nothing and closes the panel', async () => {
    const ipc = mockVaultIpc({ cloudProvider: 'icloud' })
    renderWithProviders(<MemoryVaultSettings />)

    const input = await screen.findByLabelText(copy.pathInputLabel)
    fireEvent.change(input, { target: { value: '/Users/tester/iCloud/vault' } })
    fireEvent.click(screen.getByTestId('vault-propose-path'))
    await waitFor(() => expect(screen.getByTestId('vault-path-warning')).toBeInTheDocument())

    fireEvent.click(screen.getByText(copy.warningCancel))
    await waitFor(() => expect(screen.queryByTestId('vault-path-warning')).not.toBeInTheDocument())
    expect(ipc.of('set_vault_mirror_path').filter((c) => c.args?.acknowledged === true)).toHaveLength(0)
  })

  it('surfaces a configured-but-unacknowledged path as still using the default folder', async () => {
    mockVaultIpc({
      settings: {
        custom_path: '/srv/notes/vault',
        custom_path_acknowledged: false,
        active_path: DEFAULT_PATH,
      },
    })
    renderWithProviders(<MemoryVaultSettings />)

    await waitFor(() => expect(screen.getByTestId('vault-active-path')).toHaveTextContent(DEFAULT_PATH))
    expect(screen.getByText(copy.pendingPathNote.replace('{{path}}', '/srv/notes/vault'))).toBeInTheDocument()
  })

  it('shows the standing ledger notice for an acknowledged cloud-synced path', async () => {
    mockVaultIpc({
      settings: {
        custom_path: '/Users/tester/Dropbox/vault',
        custom_path_acknowledged: true,
        active_path: '/Users/tester/Dropbox/vault',
        cloud_provider: 'dropbox',
      },
    })
    renderWithProviders(<MemoryVaultSettings />)

    await waitFor(() =>
      expect(screen.getByTestId('vault-cloud-active-notice')).toHaveTextContent(
        copy.cloudActiveNotice.replace('{{provider}}', copy.provider.dropbox),
      ),
    )
  })

  it('reverting to the default folder clears the custom path', async () => {
    const ipc = mockVaultIpc({
      settings: { custom_path: '/srv/notes/vault', custom_path_acknowledged: true },
    })
    renderWithProviders(<MemoryVaultSettings />)

    fireEvent.click(await screen.findByTestId('vault-use-default'))
    await waitFor(() => {
      const clears = ipc.of('set_vault_mirror_path').filter((c) => c.args?.path === null)
      expect(clears).toHaveLength(1)
      expect(clears[0].args).toEqual({ path: null, acknowledged: false })
    })
  })
})

describe('MemoryVaultSettings — Export now', () => {
  it('invokes a full mirror cycle and reports the counts', async () => {
    const toastSpy = vi.spyOn(toastModule, 'addToast')
    const ipc = mockVaultIpc()
    renderWithProviders(<MemoryVaultSettings />)

    fireEvent.click(await screen.findByTestId('vault-export-now'))

    await waitFor(() => expect(ipc.of('run_vault_mirror_cycle')).toHaveLength(1))
    expect(toastSpy).toHaveBeenCalledWith(
      'success',
      copy.exportDone.replace('{{days}}', '3').replace('{{expired}}', '1'),
    )
    toastSpy.mockRestore()
  })

  it('reports a fail-closed no-op cycle as skipped rather than as a successful export', async () => {
    // A skipped cycle is a SUCCESSFUL invoke with every counter zero. Calling
    // that "exported" would tell the user files exist that do not.
    const toastSpy = vi.spyOn(toastModule, 'addToast')
    mockVaultIpc({
      cycle: {
        skipped_reason: 'consent_missing',
        day_files_written: 0,
        claims_file_written: false,
        files_expired: 0,
      },
    })
    renderWithProviders(<MemoryVaultSettings />)

    fireEvent.click(await screen.findByTestId('vault-export-now'))

    await waitFor(() =>
      expect(toastSpy).toHaveBeenCalledWith('warning', copy.exportSkipped.replace('{{reason}}', 'consent_missing')),
    )
    expect(toastSpy).not.toHaveBeenCalledWith('success', expect.anything())
    toastSpy.mockRestore()
  })

  it('surfaces §6.4 marker conflicts so a missing file is explainable', async () => {
    const toastSpy = vi.spyOn(toastModule, 'addToast')
    mockVaultIpc({ cycle: { conflicts: 2 } })
    renderWithProviders(<MemoryVaultSettings />)

    fireEvent.click(await screen.findByTestId('vault-export-now'))

    await waitFor(() =>
      expect(toastSpy).toHaveBeenCalledWith('warning', copy.conflictsNotice.replace('{{count}}', '2')),
    )
    toastSpy.mockRestore()
  })

  it('maps an IPC failure through the established error mapping', async () => {
    const toastSpy = vi.spyOn(toastModule, 'addToast')
    mockVaultIpc({
      failWith: {
        cmd: 'run_vault_mirror_cycle',
        // A registry code (ADR-019) — translateError resolves it from
        // wire-errors.en.json, so the user never sees the raw Rust literal.
        error: { code: 'storage.failed', message: 'Storage error [storage.failed]: disk full' },
      },
    })
    renderWithProviders(<MemoryVaultSettings />)

    fireEvent.click(await screen.findByTestId('vault-export-now'))

    await waitFor(() => expect(toastSpy).toHaveBeenCalledWith('error', copy.exportFailed))
    const alert = await screen.findByRole('alert')
    // `describeIpcError` routed the registry code through the localized
    // wire-error catalog: the sentence carries the catalog prefix, not just the
    // raw Rust Display string the command threw.
    expect(alert.textContent).toContain(wireErrors['storage.failed'].split('{')[0].trim())
    toastSpy.mockRestore()
  })
})

describe('MemoryVaultSettings — §6.4 persisted last cycle (#9522)', () => {
  it('reports the conflicts of a cycle nobody pressed a button for, by name', async () => {
    // The scenario the issue names: a SCHEDULED cycle skipped a pre-existing
    // Obsidian daily note. No `run_vault_mirror_cycle` happens in this test —
    // the settings read alone must surface it, or the skip stays invisible.
    const ipc = mockVaultIpc({
      settings: {
        last_cycle: lastCycle({ conflicts: 2, conflict_paths: ['daily/2026-07-28.md', 'daily/2026-07-29.md'] }),
      },
    })
    renderWithProviders(<MemoryVaultSettings />)

    const panel = await screen.findByTestId('vault-last-cycle-conflicts')
    expect(panel).toHaveTextContent('daily/2026-07-28.md')
    expect(panel).toHaveTextContent('daily/2026-07-29.md')
    // The §6.4 explanation: skipped, NOT overwritten, and what to do about it.
    expect(panel).toHaveTextContent(copy.conflictsExplain)
    expect(panel).toHaveTextContent(copy.conflictsTitle.replace('{{count}}', '2'))
    expect(ipc.of('run_vault_mirror_cycle')).toHaveLength(0)
  })

  it('reports how many conflicts the capped list is hiding', async () => {
    mockVaultIpc({
      settings: { last_cycle: lastCycle({ conflicts: 25, conflict_paths: ['daily/2026-07-01.md'] }) },
    })
    renderWithProviders(<MemoryVaultSettings />)

    const panel = await screen.findByTestId('vault-last-cycle-conflicts')
    expect(panel).toHaveTextContent(copy.conflictsMore.replace('{{count}}', '24'))
  })

  it('shows the last run with its counts and no conflict panel when there were none', async () => {
    mockVaultIpc({ settings: { last_cycle: lastCycle({ day_files_written: 4, files_expired: 2 }) } })
    renderWithProviders(<MemoryVaultSettings />)

    const summary = await screen.findByTestId('vault-last-cycle-summary')
    // Counts are rendered from the PERSISTED row, not from a cycle this
    // session ran (there was none) — the assertion that fails if the
    // component ever reads them from `run_vault_mirror_cycle` instead.
    expect(summary).toHaveTextContent('4')
    expect(summary).toHaveTextContent('2')
    expect(screen.queryByTestId('vault-last-cycle-conflicts')).not.toBeInTheDocument()
  })

  it('says the mirror has not run yet rather than showing a zero-count cycle', async () => {
    mockVaultIpc({ settings: { last_cycle: null } })
    renderWithProviders(<MemoryVaultSettings />)

    expect(await screen.findByText(copy.lastCycleNever)).toBeInTheDocument()
    expect(screen.queryByTestId('vault-last-cycle-summary')).not.toBeInTheDocument()
    expect(screen.queryByTestId('vault-last-cycle-conflicts')).not.toBeInTheDocument()
  })
})

describe('MemoryVaultSettings — degraded environments', () => {
  it('renders nothing when the vault IPC is unavailable (standalone build)', async () => {
    mockInvoke.mockRejectedValue('no tauri')
    const { container } = renderWithProviders(<MemoryVaultSettings />)
    await waitFor(() => expect(container.querySelector('#section-memory-vault')).toBeNull())
  })

  it('warns that every cycle is a no-op when the window violates its bound', async () => {
    mockVaultIpc({ settings: { mirror_window_days: 120, window_within_bound: false } })
    renderWithProviders(<MemoryVaultSettings />)

    await waitFor(() => expect(screen.getByText(copy.windowOutOfBound.replace('{{days}}', '120'))).toBeInTheDocument())
  })
})
