/**
 * #9603 WD-02.1 — `/login` sign-in screen.
 *
 * The four states this asserts are the four the demo can actually hit: the
 * default (`server` feature absent) build, the connected build before sign-in,
 * a rejected credential, and the successful navigation that opens demo beat 1.
 *
 * Mock strategy mirrors `setting-tabs/GeneralTab.test.tsx`: `vi.mock`
 * intercepts the module registry for the dynamic `@tauri-apps/api/core` import
 * in `components/auth/authIpc.ts`, and `window.__TAURI_INTERNALS__` is stubbed
 * as a fallback in case the real module is resolved by a concurrent import.
 *
 * Navigation is asserted by *rendering the destination route* rather than
 * spying on `useNavigate`: a spy would pass even if the page navigated to a
 * path that renders nothing, which is precisely the regression that matters
 * when WD-02.3 later repoints `POST_LOGIN_DESTINATION` at the home view.
 */

import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import * as toastModule from '../../hooks/useToast'
import LoginPage from './LoginPage'

const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

/** Wire shape of `AuthStatusResponse` — snake_case verbatim (no serde rename). */
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

const SIGNED_IN: AuthStatus = {
  server_feature: true,
  authenticated: true,
  identifier: 'mingyu_song',
  organization_id: 'org-e2e-futurepac',
}

/**
 * Markers for the two exits, so navigation is observed as a real route change
 * — and so the two are told apart.
 *
 * #9611 WD-02.3 split them: a successful sign-in lands on `/home` (the context
 * home), while "continue without signing in" still lands on `/` (the
 * local-first dashboard, which works with no account). One marker for both
 * would let a regression that sent an unauthenticated user to `/home` pass.
 */
const DASHBOARD_MARKER = 'dashboard-overview-landed'
const CONTEXT_HOME_MARKER = 'context-home-landed'

function renderLoginRoute() {
  return renderWithProviders(
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route path="/" element={<div>{DASHBOARD_MARKER}</div>} />
      <Route path="/home" element={<div>{CONTEXT_HOME_MARKER}</div>} />
    </Routes>,
    { routerProps: { initialEntries: ['/login'] } },
  )
}

async function fillCredentials() {
  // en.json: settings.account.login.*Label — the same keys the Settings form
  // uses, because it is the same component.
  const identifier = await screen.findByLabelText(/identifier/i)
  fireEvent.change(identifier, { target: { value: 'mingyu_song' } })
  fireEvent.change(screen.getByLabelText(/password/i), { target: { value: 'pw' } })
  fireEvent.change(screen.getByLabelText(/organization id/i), { target: { value: 'org-e2e-futurepac' } })
}

describe('LoginPage (#9603 WD-02.1)', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  it('renders the three-field sign-in form when the connected build is unauthenticated', async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return SIGNED_OUT
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderLoginRoute()

    expect(await screen.findByLabelText(/identifier/i)).toBeInTheDocument()
    expect(screen.getByLabelText(/password/i)).toBeInTheDocument()
    expect(screen.getByLabelText(/organization id/i)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /^sign in$/i })).toBeInTheDocument()

    // The page is a destination, not a wall: a local-first product must stay
    // fully usable without an account, so the way past it is always on screen.
    // en.json: login.continueWithoutSignIn
    expect(screen.getByRole('button', { name: /continue without signing in/i })).toBeInTheDocument()
  })

  it('lands on the context home after a successful sign-in', async () => {
    let current: AuthStatus = SIGNED_OUT
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return current
      if (cmd === 'login') {
        current = SIGNED_IN
        return SIGNED_IN
      }
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderLoginRoute()
    await fillCredentials()

    // The destination is not rendered before the sign-in — otherwise the
    // assertion below would hold for a page that never navigated at all.
    expect(screen.queryByText(CONTEXT_HOME_MARKER)).not.toBeInTheDocument()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^sign in$/i }))
    })

    expect(await screen.findByText(CONTEXT_HOME_MARKER)).toBeInTheDocument()
    // A signed-in operator must NOT be dropped on the local-first dashboard —
    // that was the pre-#9611 destination and is what this assertion pins.
    expect(screen.queryByText(DASHBOARD_MARKER)).not.toBeInTheDocument()
    // The sign-in screen is gone, so the password field is not left mounted.
    expect(screen.queryByLabelText(/password/i)).not.toBeInTheDocument()

    // camelCase on the JS side; Tauri v2 maps them to the command's snake_case
    // parameters. Pinned here because this page is the second caller of `login`
    // and a divergent arg name would fail only at runtime.
    const loginCall = mockInvoke.mock.calls.find((call) => call[0] === 'login')
    expect(loginCall?.[1]).toEqual({
      identifier: 'mingyu_song',
      password: 'pw',
      organizationId: 'org-e2e-futurepac',
    })
  })

  it('shows the not-available notice instead of a form when the build lacks the server feature', async () => {
    mockInvoke.mockResolvedValue({
      server_feature: false,
      authenticated: false,
      identifier: null,
      organization_id: null,
    } satisfies AuthStatus)

    renderLoginRoute()

    // en.json: settings.account.login.notAvailable — the same sentence a
    // `auth.feature_disabled` rejection maps to, not a second copy of it.
    await screen.findByText(/not included in this build/i)
    expect(screen.queryByLabelText(/identifier/i)).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /^sign in$/i })).not.toBeInTheDocument()

    // Degrades honestly rather than dead-ending: the route still offers the
    // way onward that every other state does.
    expect(screen.getByRole('button', { name: /continue without signing in/i })).toBeInTheDocument()
  })

  it('degrades to the not-available notice when the IPC bridge is unreachable', async () => {
    // The standalone browser dashboard: the dynamic import / invoke rejects
    // outright. Offering a form that could never submit would be the lie.
    mockInvoke.mockRejectedValue(new Error('no tauri runtime'))

    renderLoginRoute()

    await screen.findByText(/not included in this build/i)
    expect(screen.queryByLabelText(/identifier/i)).not.toBeInTheDocument()
  })

  it('surfaces a rejected credential as the mapped sentence and stays on the form', async () => {
    const addToastSpy = vi.spyOn(toastModule, 'addToast')
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return SIGNED_OUT
      // Verbatim two-layer wire shape: `CoreError::Auth`'s Display wrapping
      // `login_with_org`'s own `login failure ({status}): {body}` format.
      if (cmd === 'login')
        throw {
          code: 'auth.failed',
          message:
            'Authentication error [auth.failed]: login failure (401 Unauthorized): {"type":"about:blank","title":"Unauthorized","status":401}',
        }
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderLoginRoute()
    await fillCredentials()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^sign in$/i }))
    })

    // en.json settings.account.login.errors.invalidCredentials, wrapped by
    // settings.account.login.error = "Sign-in failed: {{error}}".
    await waitFor(() => {
      expect(addToastSpy).toHaveBeenCalledWith(
        'error',
        'Sign-in failed: Sign-in was rejected. Check your identifier, password, and organization ID, then try again.',
      )
    })
    // The RFC 7807 body must never reach the user.
    expect(addToastSpy).not.toHaveBeenCalledWith('error', expect.stringContaining('about:blank'))
    // A failed sign-in does not navigate — the operator can correct and retry.
    expect(screen.queryByText(DASHBOARD_MARKER)).not.toBeInTheDocument()
    expect(screen.queryByText(CONTEXT_HOME_MARKER)).not.toBeInTheDocument()
    expect(screen.getByLabelText(/identifier/i)).toBeInTheDocument()
    expect(screen.getByLabelText(/password/i)).toBeInTheDocument()
  })

  it('sends "continue without signing in" to the local-first dashboard, not the context home', async () => {
    // #9611 WD-02.3: the context home has nothing to show a signed-out user —
    // its only honest render there is an error. Sharing one destination between
    // the two exits is exactly how that regression would ship.
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return SIGNED_OUT
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderLoginRoute()
    await screen.findByLabelText(/identifier/i)

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /continue without signing in/i }))
    })

    expect(await screen.findByText(DASHBOARD_MARKER)).toBeInTheDocument()
    expect(screen.queryByText(CONTEXT_HOME_MARKER)).not.toBeInTheDocument()
  })

  it('shows who is signed in and offers the way onward when a session is already restored', async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'auth_status') return SIGNED_IN
      throw new Error(`unexpected cmd ${cmd}`)
    })

    renderLoginRoute()

    await screen.findByText('Signed in as mingyu_song @ org-e2e-futurepac')
    expect(screen.queryByLabelText(/password/i)).not.toBeInTheDocument()

    // en.json: login.continueToHome — renamed with its destination (#9611).
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /continue to your context/i }))
    })
    expect(await screen.findByText(CONTEXT_HOME_MARKER)).toBeInTheDocument()
  })
})
