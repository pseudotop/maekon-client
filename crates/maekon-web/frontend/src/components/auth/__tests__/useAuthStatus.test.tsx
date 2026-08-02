/**
 * #9603 WD-02.1 — the `auth_status` read guarantees, pinned at the hook.
 *
 * Guarantee 2 in `useAuthStatus`'s doc comment ("a rejected read never
 * downgrades an already-resolved status") had no test before this file:
 * replacing `setStatus((prev) => prev ?? SERVER_UNREACHABLE)` with an
 * unconditional `setStatus(SERVER_UNREACHABLE)` left every component-level
 * suite green. That was survivable while the branch lived inside
 * `AccountSection`; it is not now that two surfaces share it and the failure is
 * user-visible — one transient IPC rejection would flip a signed-in operator's
 * Settings (and the `/login` screen) to "connected mode is not included in this
 * build", which is a lie about the binary they are running.
 *
 * Asserted at the hook rather than through a component because the hook is
 * where the decision lives; a component test would also have to keep a
 * particular rendering of that state true.
 */

import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AuthStatus } from '../authIpc'
import { useAuthStatus } from '../useAuthStatus'

const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

const SIGNED_IN: AuthStatus = {
  server_feature: true,
  authenticated: true,
  identifier: 'mingyu_song',
  organization_id: 'org-e2e-futurepac',
}

describe('useAuthStatus', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  it('resolves the mount read into status', async () => {
    mockInvoke.mockResolvedValue(SIGNED_IN)

    const { result } = renderHook(() => useAuthStatus())

    await waitFor(() => expect(result.current.status).toEqual(SIGNED_IN))
  })

  it('records the unreachable fallback when the very first read rejects', async () => {
    // The standalone browser dashboard: no IPC bridge at all. Recording
    // `server_feature: false` is what makes both surfaces render the
    // "not in this build" notice instead of an unsubmittable form.
    mockInvoke.mockRejectedValue(new Error('no tauri runtime'))

    const { result } = renderHook(() => useAuthStatus())

    await waitFor(() => expect(result.current.status).not.toBeNull())
    expect(result.current.status).toEqual({
      server_feature: false,
      authenticated: false,
      identifier: null,
      organization_id: null,
    })
  })

  it('does not downgrade an already-resolved status when a later read rejects', async () => {
    // `auth_status` is infallible Rust-side, so a rejection after a successful
    // read means something transient — not "this build lost connected mode".
    mockInvoke.mockResolvedValue(SIGNED_IN)

    const { result } = renderHook(() => useAuthStatus())
    await waitFor(() => expect(result.current.status).toEqual(SIGNED_IN))

    mockInvoke.mockRejectedValue(new Error('transient IPC failure'))
    await act(async () => {
      await result.current.refresh()
    })

    // The mutation this pins: an unconditional `setStatus(SERVER_UNREACHABLE)`
    // in the catch tells a signed-in operator their build has no connected mode.
    expect(result.current.status).toEqual(SIGNED_IN)
  })

  it('re-reads when the document returns to the foreground', async () => {
    // Both surfaces can sit open, hidden, for hours while the session expires or
    // is revoked from another device.
    mockInvoke.mockResolvedValue(SIGNED_IN)

    renderHook(() => useAuthStatus())
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('auth_status', undefined))

    const readsBefore = mockInvoke.mock.calls.length
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'))
    })

    await waitFor(() => expect(mockInvoke.mock.calls.length).toBeGreaterThan(readsBefore))
  })

  it('drops the visibilitychange listener on unmount', async () => {
    mockInvoke.mockResolvedValue(SIGNED_IN)

    const { unmount } = renderHook(() => useAuthStatus())
    await waitFor(() => expect(mockInvoke).toHaveBeenCalled())

    unmount()
    const readsAfterUnmount = mockInvoke.mock.calls.length

    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'))
    })

    expect(mockInvoke.mock.calls.length).toBe(readsAfterUnmount)
  })
})
