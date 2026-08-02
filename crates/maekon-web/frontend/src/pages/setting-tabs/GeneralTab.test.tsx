import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import * as toastModule from '../../hooks/useToast'
import { AccountSection, StartupSection } from './GeneralTab'

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

  it('toggle click invokes disable_autostart when turning off', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'is_autostart_enabled') return Promise.resolve(true)
      if (cmd === 'autostart_capabilities') return Promise.resolve({ supported: true, environment: 'mac_os' })
      if (cmd === 'disable_autostart') return Promise.resolve(undefined)
      return Promise.resolve(undefined)
    })

    renderWithProviders(<StartupSection />)

    const toggle = await screen.findByRole('checkbox', { name: /Start Maekon at login/i })
    await waitFor(() => expect(toggle).toBeChecked())
    await waitFor(() => expect(toggle).not.toBeDisabled())

    await act(async () => {
      fireEvent.click(toggle)
    })

    await waitFor(() => {
      expect(mockInvoke.mock.calls.some((call) => call[0] === 'disable_autostart')).toBe(true)
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

// ---------------------------------------------------------------------------
// Tests — AccountSection sign-in form (#9459)
//
// Wire shape of `AuthStatusResponse` is snake_case verbatim (no serde rename);
// it is pinned Rust-side by
// `src-tauri/src/commands/auth.rs::auth_status_response_serializes_exact_snake_case_wire_keys`.
// ---------------------------------------------------------------------------

interface AuthStatus {
  server_feature: boolean
  authenticated: boolean
  identifier: string | null
  organization_id: string | null
}

const SIGNED_OUT: AuthStatus = {
  server_feature: true,
  authenticated: false,
  identifier: null,
  organization_id: null,
}

describe('GeneralTab — Account sign-in form', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  it('renders the sign-in form when unauthenticated and submits credentials', async () => {
    const signedIn: AuthStatus = {
      server_feature: true,
      authenticated: true,
      identifier: 'mingyu_song',
      organization_id: 'org-e2e-futurepac',
    }
    // #9492 item 2: `auth_status` is re-read after a successful login, so the
    // stub has to behave like the real command and report the session the login
    // just created — not a frozen snapshot.
    let current: AuthStatus = SIGNED_OUT
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return current
      if (cmd === 'login') {
        current = signedIn
        return signedIn
      }
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderWithProviders(<AccountSection />)

    // en.json: settings.account.login.identifierLabel = "Identifier"
    const identifier = await screen.findByLabelText(/identifier/i)
    const password = screen.getByLabelText(/password/i)
    const organizationId = screen.getByLabelText(/organization id/i)

    fireEvent.change(identifier, { target: { value: 'mingyu_song' } })
    fireEvent.change(password, { target: { value: 'pw' } })
    fireEvent.change(organizationId, { target: { value: 'org-e2e-futurepac' } })

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^sign in$/i }))
    })

    // en.json: settings.account.login.signedInAs
    await screen.findByText('Signed in as mingyu_song @ org-e2e-futurepac')

    // IPC arg names are camelCase on the JS side — Tauri v2 converts them to the
    // snake_case parameters of `login(identifier, password, organization_id)`.
    const loginCall = mockInvoke.mock.calls.find((call) => call[0] === 'login')
    expect(loginCall?.[1]).toEqual({
      identifier: 'mingyu_song',
      password: 'pw',
      organizationId: 'org-e2e-futurepac',
    })

    // The form is replaced by the signed-in summary, so the password never
    // lingers in a mounted input.
    expect(screen.queryByLabelText(/password/i)).not.toBeInTheDocument()

    // #9492 item 2: mount read + post-login re-read. The displayed summary is
    // server truth, not just whatever `login` happened to return.
    const statusCalls = mockInvoke.mock.calls.filter((call) => call[0] === 'auth_status')
    expect(statusCalls.length).toBeGreaterThanOrEqual(2)
  })

  it('shows the not-available notice when the build lacks the server feature', async () => {
    mockInvoke.mockResolvedValue({
      server_feature: false,
      authenticated: false,
      identifier: null,
      organization_id: null,
    } satisfies AuthStatus)

    renderWithProviders(<AccountSection />)

    // en.json: settings.account.login.notAvailable
    await screen.findByText(/not included in this build/i)
    expect(screen.queryByLabelText(/identifier/i)).not.toBeInTheDocument()
  })

  it('surfaces login failure via error toast and keeps the form', async () => {
    const addToastSpy = vi.spyOn(toastModule, 'addToast')
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return SIGNED_OUT
      // A transport failure, not a rejected credential: `login_with_org` mints
      // `auth.failed` for both, so the form copy must not claim "wrong password".
      // Message is the real two-layer shape (see LOGIN_FAILURE_401's doc):
      // `CoreError::Auth`'s Display wrapping `login_with_org`'s own format.
      if (cmd === 'login')
        throw {
          code: 'auth.failed',
          message:
            'Authentication error [auth.failed]: login request failure: error sending request for url (http://127.0.0.1:8000/api/v1/auth/tokens)',
        }
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderWithProviders(<AccountSection />)

    const identifier = await screen.findByLabelText(/identifier/i)
    fireEvent.change(identifier, { target: { value: 'mingyu_song' } })
    fireEvent.change(screen.getByLabelText(/password/i), { target: { value: 'wrong' } })
    fireEvent.change(screen.getByLabelText(/organization id/i), { target: { value: 'org-e2e-futurepac' } })

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^sign in$/i }))
    })

    // en.json settings.account.login.errors.rejected, wrapped by
    // settings.account.login.error = "Sign-in failed: {{error}}".
    await waitFor(() => {
      expect(addToastSpy).toHaveBeenCalledWith(
        'error',
        'Sign-in failed: Sign-in failed. The server could not be reached or refused the request — try again in a moment.',
      )
    })
    // #9492 item 1: the Rust literal must not reach the user even on this branch.
    expect(addToastSpy).not.toHaveBeenCalledWith('error', expect.stringContaining('connection refused'))
    expect(screen.getByLabelText(/identifier/i)).toBeInTheDocument()
    expect(screen.getByLabelText(/password/i)).toBeInTheDocument()
  })

  it('replaces the 401 RFC 7807 body with a localized sentence (#9492 item 1)', async () => {
    const addToastSpy = vi.spyOn(toastModule, 'addToast')
    // Verbatim wire shape, both formatting layers included: `login_with_org`
    // builds `login failure ({status}): {length-capped body}`, then
    // `IpcError::from` stores `CoreError::Auth`'s Display, which prefixes
    // `Authentication error [{code}]: `. Pinned Rust-side by
    // `commands::auth::tests::ipc_error_message_shape_matches_frontend_401_matcher`;
    // the same nesting is documented for the chat surface in
    // `pages/chat/providerErrorGuidance.test.ts`.
    const rustMessage =
      'Authentication error [auth.failed]: login failure (401 Unauthorized): ' +
      '{"type":"https://maekon.dev/errors/authentication-failed",' +
      '"title":"Authentication failed","status":401,"detail":"Invalid identifier or password"}'
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return SIGNED_OUT
      if (cmd === 'login') throw { code: 'auth.failed', message: rustMessage }
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderWithProviders(<AccountSection />)

    const identifier = await screen.findByLabelText(/identifier/i)
    fireEvent.change(identifier, { target: { value: 'mingyu_song' } })
    fireEvent.change(screen.getByLabelText(/password/i), { target: { value: 'wrong' } })
    fireEvent.change(screen.getByLabelText(/organization id/i), { target: { value: 'org-e2e-futurepac' } })

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^sign in$/i }))
    })

    await waitFor(() => expect(addToastSpy).toHaveBeenCalledWith('error', expect.any(String)))
    const [, toastText] = addToastSpy.mock.calls[addToastSpy.mock.calls.length - 1] as [string, string]

    // en.json settings.account.login.errors.invalidCredentials.
    expect(toastText).toContain('Check your identifier, password, and organization ID')
    // Neither the status line nor any fragment of the JSON problem document.
    expect(toastText).not.toContain('401')
    expect(toastText).not.toContain('{')
    expect(toastText).not.toContain('https://')
    // …nor the `CoreError` Display wrapper the message arrives inside.
    expect(toastText).not.toContain('Authentication error')
  })

  it('degrades to the not-available notice when the IPC bridge is unreachable', async () => {
    // Outside Tauri the dynamic `@tauri-apps/api/core` import (or the invoke
    // itself) rejects. That must land on the same read-only notice a non-server
    // build gets — never a form whose submit could not possibly work.
    mockInvoke.mockRejectedValue(new Error('no tauri'))

    renderWithProviders(<AccountSection />)

    await screen.findByText(/not included in this build/i)
    expect(screen.queryByLabelText(/identifier/i)).not.toBeInTheDocument()
  })

  it('localizes auth codes that are absent from the wire-error registry', async () => {
    // `auth.invalid_arguments` is an IpcError literal minted in
    // src-tauri/src/commands/auth.rs, not a CoreError variant, so it is absent
    // from wire_contract_snapshot.expected.txt and translateError would fall
    // back to the raw English Rust message.
    const addToastSpy = vi.spyOn(toastModule, 'addToast')
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return SIGNED_OUT
      if (cmd === 'login')
        throw {
          code: 'auth.invalid_arguments',
          message: 'identifier, password, and organization id are all required',
        }
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderWithProviders(<AccountSection />)

    const identifier = await screen.findByLabelText(/identifier/i)
    fireEvent.change(identifier, { target: { value: '   ' } })
    fireEvent.change(screen.getByLabelText(/password/i), { target: { value: 'pw' } })
    fireEvent.change(screen.getByLabelText(/organization id/i), { target: { value: 'org-e2e-futurepac' } })

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^sign in$/i }))
    })

    await waitFor(() => {
      expect(addToastSpy).toHaveBeenCalledWith(
        'error',
        'Sign-in failed: Enter your identifier, password, and organization ID.',
      )
    })
    // The Rust literal must not reach the user.
    expect(addToastSpy).not.toHaveBeenCalledWith('error', expect.stringContaining('organization id are all required'))
  })

  it('returns to the sign-in form after signing out of all devices', async () => {
    // Stateful for the same reason as the sign-in case: `auth_status` is re-read
    // once the revoke succeeds, so the stub must stop reporting a live session.
    let current: AuthStatus = {
      server_feature: true,
      authenticated: true,
      identifier: 'mingyu_song',
      organization_id: 'org-e2e-futurepac',
    }
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return current
      if (cmd === 'logout_all_sessions') {
        current = SIGNED_OUT
        return undefined
      }
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderWithProviders(<AccountSection />)

    await screen.findByText('Signed in as mingyu_song @ org-e2e-futurepac')
    fireEvent.click(screen.getByRole('button', { name: /sign out of all devices/i }))

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /sign out everywhere/i }))
    })

    // The revoked session must not keep advertising an identifier.
    await screen.findByLabelText(/identifier/i)
    expect(screen.queryByText('Signed in as mingyu_song @ org-e2e-futurepac')).not.toBeInTheDocument()

    // #9492 item 2: mount read + post-revoke re-read.
    const statusCalls = mockInvoke.mock.calls.filter((call) => call[0] === 'auth_status')
    expect(statusCalls.length).toBeGreaterThanOrEqual(2)
  })

  it('re-reads auth_status when the Settings window becomes visible again (#9492 item 2)', async () => {
    // The mount-only read raced bootstrap session restore: a Settings window
    // opened early — or left open for hours while the session was revoked from
    // another device — kept showing whatever the first read happened to see.
    let current: AuthStatus = SIGNED_OUT
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return current
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderWithProviders(<AccountSection />)
    await screen.findByLabelText(/identifier/i)

    const readsBefore = mockInvoke.mock.calls.filter((call) => call[0] === 'auth_status').length
    current = {
      server_feature: true,
      authenticated: true,
      identifier: 'mingyu_song',
      organization_id: 'org-e2e-futurepac',
    }

    // jsdom reports `visibilityState === 'visible'` by default, so dispatching
    // the event is the whole foreground transition as far as the listener sees.
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'))
    })

    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter((call) => call[0] === 'auth_status').length).toBeGreaterThan(readsBefore)
    })
    await screen.findByText('Signed in as mingyu_song @ org-e2e-futurepac')
  })

  it('drops the visibilitychange listener on unmount', async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return SIGNED_OUT
      throw new Error(`unexpected cmd ${cmd}`)
    })

    const { unmount } = renderWithProviders(<AccountSection />)
    await screen.findByLabelText(/identifier/i)
    unmount()

    const readsAfterUnmount = mockInvoke.mock.calls.filter((call) => call[0] === 'auth_status').length
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'))
    })

    // A leaked listener would keep invoking IPC for an unmounted section.
    expect(mockInvoke.mock.calls.filter((call) => call[0] === 'auth_status').length).toBe(readsAfterUnmount)
  })

  it('does not let a slow auth_status read overwrite a newer one', async () => {
    // The observable half of the `useLatestOnlyRead` guard. The mount read is
    // held open, a foreground re-read starts and lands first with a live
    // session, and only then does the mount read resolve with the stale
    // signed-out snapshot. Without the guard that stale value wins and the
    // section drops back to the sign-in form for a signed-in account.
    //
    // The unmount half has no observable symptom — React 18 ignores a
    // `setState` on a detached fiber silently — so it is asserted directly
    // against the guard in `hooks/__tests__/useLatestOnlyRead.test.ts`.
    const SIGNED_IN: AuthStatus = {
      server_feature: true,
      authenticated: true,
      identifier: 'mingyu_song',
      organization_id: 'org-e2e-futurepac',
    }

    let releaseMountRead: ((status: AuthStatus) => void) | undefined
    let reads = 0
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd !== 'auth_status') throw new Error(`unexpected cmd ${cmd}`)
      reads += 1
      if (reads === 1) {
        return new Promise<AuthStatus>((resolve) => {
          releaseMountRead = resolve
        })
      }
      return Promise.resolve(SIGNED_IN)
    })

    renderWithProviders(<AccountSection />)
    await waitFor(() => expect(reads).toBe(1))

    // Foreground re-read overtakes the still-pending mount read.
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'))
    })
    await screen.findByText('Signed in as mingyu_song @ org-e2e-futurepac')

    // Now the original, stale read finally answers.
    await act(async () => {
      releaseMountRead?.(SIGNED_OUT)
    })

    expect(screen.getByText('Signed in as mingyu_song @ org-e2e-futurepac')).toBeInTheDocument()
    expect(screen.queryByLabelText(/identifier/i)).not.toBeInTheDocument()
  })
})
