import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { StartupSection } from './GeneralTab'

// ---------------------------------------------------------------------------
// Mock: @tauri-apps/api/core + window.__TAURI_INTERNALS__
//
// StartupSection's invokeDesktop() issues two *concurrent* dynamic imports
// via Promise.all.  Vitest's vi.mock intercepts the module registry, but
// concurrent dynamic imports from component code can race and occasionally
// resolve to the real @tauri-apps/api/core, which calls
// window.__TAURI_INTERNALS__.invoke — undefined in jsdom.
//
// Strategy: (1) vi.mock intercepts the module-level import, AND
// (2) stub window.__TAURI_INTERNALS__.invoke as a global fallback so that
// even if the real module resolves, the call still routes through mockInvoke.
// ---------------------------------------------------------------------------
const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

// ---------------------------------------------------------------------------
// Mock: api/client — prevent real HTTP calls from SupportToolsCard (via
// the full GeneralTab), which would otherwise error in jsdom.
// ---------------------------------------------------------------------------
vi.mock('../../api/client', () => ({
  fetchSupportDiagnostics: vi.fn().mockResolvedValue({
    schema_version: 1,
    generated_at: 'now',
    health: {},
    recent_audit_entries: [],
    recent_policy_events: [],
  }),
  fetchSettings: vi.fn().mockResolvedValue({}),
  fetchUpdateStatus: vi.fn().mockResolvedValue(null),
  fetchStorageStats: vi.fn().mockResolvedValue(null),
  fetchProviderSurfaces: vi.fn().mockResolvedValue({ surfaces: [] }),
  fetchFeatureCapabilities: vi.fn().mockResolvedValue({}),
  fetchSecretBackendCapabilities: vi.fn().mockResolvedValue({}),
  fetchDesktopPermissionStatus: vi.fn().mockResolvedValue(null),
  probeProviderSurfaceEndpoint: vi.fn().mockResolvedValue(null),
}))

// ---------------------------------------------------------------------------
// Tests — StartupSection component
// ---------------------------------------------------------------------------

describe('GeneralTab — Startup section', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    // Stub window.__TAURI_INTERNALS__ so that even if the real Tauri module
    // is resolved (bypassing vi.mock for the second concurrent dynamic import),
    // the invoke call still routes through mockInvoke.
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  it('renders Startup section with heading', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'is_autostart_enabled') return Promise.resolve(false)
      if (cmd === 'autostart_capabilities') return Promise.resolve({ supported: true, environment: 'mac_os' })
      return Promise.resolve(undefined)
    })

    renderWithProviders(<StartupSection />)

    // en.json: settings.autostart.title = "Startup"
    await waitFor(() => {
      expect(screen.getByText('Startup')).toBeInTheDocument()
    })
  })

  it('toggle initial state loads from is_autostart_enabled IPC (true → checked)', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'is_autostart_enabled') return Promise.resolve(true)
      if (cmd === 'autostart_capabilities') return Promise.resolve({ supported: true, environment: 'mac_os' })
      return Promise.resolve(undefined)
    })

    renderWithProviders(<StartupSection />)

    await waitFor(() => {
      // en.json: settings.autostart.toggle = "Start Maekon at login"
      const toggle = screen.getByRole('checkbox', {
        name: /Start Maekon at login/i,
      })
      expect(toggle).toBeChecked()
    })
  })

  it('toggle disabled when capabilities.supported = false', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'is_autostart_enabled') return Promise.resolve(false)
      if (cmd === 'autostart_capabilities')
        return Promise.resolve({
          supported: false,
          unsupported_reason: { kind: 'snap_sandbox' },
          environment: 'linux_snap_sandbox',
        })
      return Promise.resolve(undefined)
    })

    renderWithProviders(<StartupSection />)

    await waitFor(() => {
      const toggle = screen.getByRole('checkbox', {
        name: /Start Maekon at login/i,
      })
      expect(toggle).toBeDisabled()
    })
  })

  it('toggle click invokes enable_autostart when turning on', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'is_autostart_enabled') return Promise.resolve(false)
      if (cmd === 'autostart_capabilities') return Promise.resolve({ supported: true, environment: 'mac_os' })
      if (cmd === 'enable_autostart') return Promise.resolve(undefined)
      return Promise.resolve(undefined)
    })

    renderWithProviders(<StartupSection />)

    // Wait until the toggle is enabled (IPC has resolved) before clicking.
    // findByRole returns as soon as the element exists — even disabled —
    // so we must waitFor the enabled state explicitly.
    const toggle = await screen.findByRole('checkbox', { name: /Start Maekon at login/i })
    await waitFor(() => expect(toggle).not.toBeDisabled())

    await act(async () => {
      fireEvent.click(toggle)
    })

    await waitFor(() => {
      // invokeDesktop passes `args` (undefined when not supplied) as the second
      // argument to the underlying invoke, so the call shape is
      // ('enable_autostart', undefined).  Check by command name only.
      expect(mockInvoke.mock.calls.some((call) => call[0] === 'enable_autostart')).toBe(true)
    })
  })

  it('toggle error re-fetches OS state via is_autostart_enabled', async () => {
    let queryCallCount = 0
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'is_autostart_enabled') {
        queryCallCount++
        return Promise.resolve(false)
      }
      if (cmd === 'autostart_capabilities') return Promise.resolve({ supported: true, environment: 'mac_os' })
      if (cmd === 'enable_autostart') return Promise.reject(new Error('permissions denied'))
      return Promise.resolve(undefined)
    })

    renderWithProviders(<StartupSection />)

    const toggle = await screen.findByRole('checkbox', {
      name: /Start Maekon at login/i,
    })
    fireEvent.click(toggle)

    // Initial mount query + post-error re-fetch = at least 2 calls
    await waitFor(() => {
      expect(queryCallCount).toBeGreaterThanOrEqual(2)
    })
  })
})
