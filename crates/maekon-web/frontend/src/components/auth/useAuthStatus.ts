/**
 * Shared `auth_status` reader (#9603 WD-02.1).
 *
 * Extracted from `AccountSection` so the demo sign-in screen inherits the same
 * three guarantees rather than re-deriving them:
 *
 * 1. **Stale-read guard.** The mount read races bootstrap session restore, and
 *    a foreground re-read can overtake it. `useLatestOnlyRead` is what stops a
 *    slow earlier read from landing on top of a newer one (and drops any read
 *    still in flight at unmount).
 * 2. **A rejected read never downgrades a resolved status.** `auth_status` is
 *    infallible Rust-side, so a rejection means the IPC bridge is absent
 *    entirely (the standalone browser dashboard) — a permanent condition the
 *    first read already recorded as [`SERVER_UNREACHABLE`].
 * 3. **Foreground re-read.** Both surfaces can sit open, hidden, for hours
 *    while the session expires or is revoked from another device. Deliberately
 *    no polling — a hidden window displays nothing worth keeping fresh.
 */

import { type Dispatch, type SetStateAction, useCallback, useEffect, useState } from 'react'
import { useLatestOnlyRead } from '../../hooks/useLatestOnlyRead'
import { type AuthStatus, readAuthStatus, SERVER_UNREACHABLE } from './authIpc'

export interface AuthStatusState {
  /** `null` until the first read settles — render a spinner, not a form. */
  status: AuthStatus | null
  /** Re-read server truth. Safe to call concurrently; only the newest result is applied. */
  refresh: () => Promise<void>
  /**
   * Apply a locally-known status without a round trip (e.g. the session a
   * `login` call just returned, or the revocation `logout_all_sessions`
   * implies). Callers still follow up with [`refresh`] so what is displayed is
   * server truth rather than what a command implied.
   */
  setStatus: Dispatch<SetStateAction<AuthStatus | null>>
}

export function useAuthStatus(): AuthStatusState {
  const [status, setStatus] = useState<AuthStatus | null>(null)
  const read = useLatestOnlyRead()

  const refresh = useCallback(async () => {
    const token = read.begin()
    try {
      const next = await readAuthStatus()
      if (read.isCurrent(token)) setStatus(next)
    } catch {
      if (read.isCurrent(token)) setStatus((prev) => prev ?? SERVER_UNREACHABLE)
    }
  }, [read])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useEffect(() => {
    const handleVisibility = () => {
      if (document.visibilityState === 'visible') void refresh()
    }
    document.addEventListener('visibilitychange', handleVisibility)
    return () => document.removeEventListener('visibilitychange', handleVisibility)
  }, [refresh])

  return { status, refresh, setStatus }
}
