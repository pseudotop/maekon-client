/**
 * #9611 WD-02.3 — failure classification.
 *
 * `classifyFailure` is the single place that decides which of the six states a
 * rejection produces. The requirement is that they stay distinguishable, so
 * these tests assert the distinctions rather than the happy path — in
 * particular the two that are easiest to collapse and worst to get wrong:
 *
 * - an expired session vs a permission denial (one is fixed by re-login, one
 *   is not, and offering the wrong remedy is a loop with no exit)
 * - a transient failure with a previous snapshot vs without one (blanking the
 *   screen costs the user everything they were reading)
 */

import { describe, expect, it } from 'vitest'
import {
  CONTEXT_HOME_ERROR_CODES,
  ContextHomeBridgeUnavailableError,
  type ContextHomeSnapshot,
} from '../../../api/contextHome'
import { classifyFailure } from '../useContextHome'

const ipcError = (code: string) => ({ code, message: `simulated ${code}` })
const snapshot = { snapshot_id: 'prev' } as unknown as ContextHomeSnapshot

describe('classifyFailure (#9611)', () => {
  it('sends an expired session to reauth and a denial to denied — never the same state', () => {
    const expired = classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.sessionExpired), null)
    const denied = classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.permissionDenied), null)

    expect(expired.kind).toBe('reauth')
    expect(denied.kind).toBe('denied')
    expect(expired.kind).not.toBe(denied.kind)
  })

  it('does not keep showing data after a denial, even with a snapshot on screen', () => {
    // Continuing to render rows the server has just said this account may not
    // have is worse than showing nothing.
    expect(classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.permissionDenied), snapshot).kind).toBe('denied')
    expect(classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.sessionExpired), snapshot).kind).toBe('reauth')
  })

  it('degrades a transient failure to stale rather than blanking the screen', () => {
    for (const code of [CONTEXT_HOME_ERROR_CODES.unavailable, CONTEXT_HOME_ERROR_CODES.timeout]) {
      const view = classifyFailure(ipcError(code), snapshot)
      expect(view).toEqual({ kind: 'ready', snapshot, stale: true })
    }
  })

  it('reports unavailable when a transient failure has nothing to fall back to', () => {
    const view = classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.unavailable), null)
    expect(view).toEqual({ kind: 'unavailable', retryable: true })
  })

  it('keeps a malformed response out of the transient bucket', () => {
    // Retrying an unreadable contract produces the same unreadable contract;
    // presenting it as "temporary" sends the user into a pointless loop.
    expect(classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.invalidResponse), null).kind).toBe('malformed')
    expect(classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.invalidResponse), snapshot).kind).toBe('malformed')
  })

  it('separates a missing desktop bridge from a server that is down', () => {
    expect(classifyFailure(new ContextHomeBridgeUnavailableError(), null).kind).toBe('bridgeAbsent')
    expect(classifyFailure(new ContextHomeBridgeUnavailableError(), snapshot).kind).toBe('bridgeAbsent')
  })

  it('treats an unrecognised rejection as transient rather than claiming a verdict', () => {
    // Asserting "permission denied" or "session expired" on a code we do not
    // recognise would be a claim the evidence does not support.
    expect(classifyFailure(new Error('boom'), null).kind).toBe('unavailable')
    expect(classifyFailure(ipcError('some.future.code'), snapshot)).toEqual({
      kind: 'ready',
      snapshot,
      stale: true,
    })
  })

  it('produces every declared state from some input', () => {
    // A state nothing can reach is dead UI that will rot. This is the same
    // guard the server read-model carries for its unavailable-reason enum.
    const reached = new Set(
      [
        classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.sessionExpired), null),
        classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.permissionDenied), null),
        classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.invalidResponse), null),
        classifyFailure(new ContextHomeBridgeUnavailableError(), null),
        classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.unavailable), null),
        classifyFailure(ipcError(CONTEXT_HOME_ERROR_CODES.timeout), snapshot),
      ].map((v) => v.kind),
    )

    expect(reached).toEqual(new Set(['reauth', 'denied', 'malformed', 'bridgeAbsent', 'unavailable', 'ready']))
  })
})
