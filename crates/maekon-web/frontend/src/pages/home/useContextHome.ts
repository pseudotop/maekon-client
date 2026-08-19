/**
 * Context-home state machine (#9611 WD-02.3).
 *
 * Sits on the #9625 typed IPC bridge (`api/contextHome`) and turns its outcomes
 * into states the page can render **distinguishably**. That word is the whole
 * requirement: the failure this slice exists to prevent is a home view that
 * draws "nothing here" for six different situations, three of which are the
 * user's fault to fix and three of which are not.
 *
 * ## Why fetch-shape and data-shape are separate concerns
 *
 * `loading` / `unavailable` / `reauth` / `denied` describe *the request*.
 * `empty` and `partial` describe *the answer* — the server replied fine, and
 * some or all of its sections had nothing or could not be served. The latter
 * pair therefore lives on the snapshot (each section carries its own `status`),
 * not in this union. Folding them in would force this hook to re-derive what
 * the server already stated, and the two copies would disagree the first time
 * the contract grew a section.
 *
 * ## Stale is a state, not an error
 *
 * A refresh that fails while a previous snapshot is on screen must not blank the
 * screen — the old data is still the best available answer, and wiping it costs
 * the user everything they were reading. It also must not silently pretend to be
 * current. So the snapshot stays, `stale` goes true, and the page says when it
 * was taken. This is why [`ContextHomeView`] carries `snapshot` and `stale`
 * together rather than having a separate `stale` variant that would have to
 * duplicate every ready-state field.
 *
 * ## Session expiry and permission denial never merge
 *
 * `reauth` (401) is the only state that routes to `/login`. `denied` (403) is
 * authenticated-but-not-permitted, where re-login changes nothing — offering it
 * would hand the user a loop with no exit. #9625 kept these as distinct wire
 * codes precisely so this hook could keep them as distinct states.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import {
  CONTEXT_HOME_ERROR_CODES,
  ContextHomeBridgeUnavailableError,
  type ContextHomeSnapshot,
  fetchContextHome,
  isContextHomeError,
} from '../../api/contextHome'

/** How the page should render right now. */
export type ContextHomeView =
  /** First read has not settled and there is nothing to show yet. */
  | { kind: 'loading' }
  /**
   * A snapshot is on screen. `stale` means the most recent refresh failed and
   * this is the previous answer — still shown, explicitly not claimed current.
   */
  | { kind: 'ready'; snapshot: ContextHomeSnapshot; stale: boolean }
  /** Transient: server fault, timeout, or no transport wired. Retry is offered. */
  | { kind: 'unavailable'; retryable: true }
  /** The session is gone. The page routes to `/login`; re-login is the fix. */
  | { kind: 'reauth' }
  /** Authenticated but not permitted. Re-login would NOT help, so it is not offered. */
  | { kind: 'denied' }
  /** No desktop IPC bridge at all — the standalone browser dashboard. */
  | { kind: 'bridgeAbsent' }
  /** The response was not a valid snapshot. Not retryable; a contract problem. */
  | { kind: 'malformed' }

export interface ContextHomeState {
  view: ContextHomeView
  /** Re-read. Safe to call concurrently; only the newest result is applied. */
  refresh: () => Promise<void>
  /** True while a refresh is in flight *over* an existing snapshot. */
  refreshing: boolean
}

/**
 * Classify a rejection into a view.
 *
 * Exported so the table is testable without mounting anything — the
 * distinctions are the requirement, and a test that needed a rendered tree to
 * check them would cover far less of the table.
 *
 * `previous` decides only one thing: whether a transient failure blanks the
 * screen or degrades to stale. Non-transient failures (reauth, denied,
 * malformed) replace the view regardless — continuing to show data the server
 * has just said you may not have is worse than showing nothing.
 */
export function classifyFailure(err: unknown, previous: ContextHomeSnapshot | null): ContextHomeView {
  if (err instanceof ContextHomeBridgeUnavailableError) return { kind: 'bridgeAbsent' }
  if (isContextHomeError(err, CONTEXT_HOME_ERROR_CODES.sessionExpired)) return { kind: 'reauth' }
  if (isContextHomeError(err, CONTEXT_HOME_ERROR_CODES.permissionDenied)) return { kind: 'denied' }
  if (isContextHomeError(err, CONTEXT_HOME_ERROR_CODES.invalidResponse)) return { kind: 'malformed' }

  // Transient (service.unavailable / network.timeout) and anything unrecognised.
  // An unknown rejection is treated as transient on purpose: the alternative is
  // claiming a permanent verdict the code did not support.
  if (previous) return { kind: 'ready', snapshot: previous, stale: true }
  return { kind: 'unavailable', retryable: true }
}

export function useContextHome(): ContextHomeState {
  const [view, setView] = useState<ContextHomeView>({ kind: 'loading' })
  const [refreshing, setRefreshing] = useState(false)

  // The last good snapshot, kept outside React state so `refresh` does not have
  // to depend on the view it replaces — a dependency that would rebuild the
  // callback on every fetch and re-fire the mount effect.
  const lastSnapshot = useRef<ContextHomeSnapshot | null>(null)
  // Monotonic token: a slow earlier read must never land on top of a newer one.
  // The home is re-read on window focus, so overlapping reads are the norm, not
  // an edge case.
  const readToken = useRef(0)
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  const refresh = useCallback(async () => {
    const token = ++readToken.current
    if (lastSnapshot.current) setRefreshing(true)
    try {
      const snapshot = await fetchContextHome()
      if (!mounted.current || token !== readToken.current) return
      lastSnapshot.current = snapshot
      setView({ kind: 'ready', snapshot, stale: false })
    } catch (err) {
      if (!mounted.current || token !== readToken.current) return
      setView(classifyFailure(err, lastSnapshot.current))
    } finally {
      if (mounted.current && token === readToken.current) setRefreshing(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useEffect(() => {
    // Re-read when the window comes back to the foreground. Deliberately no
    // polling: this window can sit hidden for hours, and nothing about a hidden
    // window is worth keeping fresh. Mirrors `useAuthStatus`.
    const onVisible = () => {
      if (document.visibilityState === 'visible') void refresh()
    }
    document.addEventListener('visibilitychange', onVisible)
    return () => document.removeEventListener('visibilitychange', onVisible)
  }, [refresh])

  return { view, refresh, refreshing }
}
