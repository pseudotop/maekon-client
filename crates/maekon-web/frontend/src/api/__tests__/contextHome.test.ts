/**
 * #9625 — context-home IPC wrapper.
 *
 * The transport policy (status mapping, body cap, bearer handling) is Rust-side
 * and tested there (`crates/maekon-network/src/context_home.rs`); a second copy
 * here could only disagree with the enforcing one. What this suite covers is
 * what only the TypeScript side can get wrong: the command name, the fact that
 * the call carries no identity argument, and the failure classification the UI
 * (#9611) branches on — in particular that "session expired" and "not
 * permitted" stay separable all the way to the caller.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
// The client-subtree mirror, not the monorepo original: `clients/maekon-client`
// is exported verbatim to the public repo, so a path that climbed out of it
// would compile here and fail there. `scripts/check-context-home-fixture-sync.sh`
// is what keeps the mirror honest.
import fixture from '../../../../../../api/fixtures/context-home.v1.json'
import {
  CONTEXT_HOME_CONTRACT_VERSION,
  CONTEXT_HOME_ERROR_CODES,
  ContextHomeBridgeUnavailableError,
  type ContextHomeSnapshot,
  fetchContextHome,
  isContextHomeError,
  isRetryableContextHomeError,
} from '../contextHome'

const mockInvoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

const snapshot = fixture as unknown as ContextHomeSnapshot

describe('fetchContextHome (#9625)', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockInvoke.mockResolvedValue(snapshot)
  })

  it('invokes the registered command with no arguments at all', async () => {
    // The moment a single argument appears, "fetch someone else's home"
    // becomes expressible from this side of the boundary.
    await fetchContextHome()

    expect(mockInvoke).toHaveBeenCalledTimes(1)
    expect(mockInvoke).toHaveBeenCalledWith('fetch_context_home')
    const [, args] = mockInvoke.mock.calls[0]
    expect(args).toBeUndefined()
  })

  it('returns the snapshot unchanged rather than reshaping it', async () => {
    const result = await fetchContextHome()
    expect(result).toEqual(snapshot)
  })

  it('does not retry — the caller owns that policy', async () => {
    mockInvoke.mockRejectedValueOnce({
      code: CONTEXT_HOME_ERROR_CODES.timeout,
      message: 'Request timed out [network.timeout] after 0ms',
    })

    await expect(fetchContextHome()).rejects.toMatchObject({
      code: CONTEXT_HOME_ERROR_CODES.timeout,
    })
    expect(mockInvoke).toHaveBeenCalledTimes(1)
  })

  it('surfaces the Rust rejection envelope unchanged', async () => {
    const envelope = {
      code: CONTEXT_HOME_ERROR_CODES.permissionDenied,
      message: 'Policy denied [policy.denied]: context home access denied for this actor',
    }
    mockInvoke.mockRejectedValueOnce(envelope)

    await expect(fetchContextHome()).rejects.toEqual(envelope)
  })
})

describe('the committed fixture and these types agree (#9625)', () => {
  it('parses as the shape this module declares', () => {
    // The server generates this from its own DTOs and byte-compares it in CI.
    // If it stops parsing here, the contract has split.
    expect(snapshot.contract_version).toBe(CONTEXT_HOME_CONTRACT_VERSION)
    expect(snapshot.actor.actor_id).toBeTruthy()
    expect(snapshot.actor.organization_id).toBeTruthy()
    expect(snapshot.timezone).toBe('Asia/Seoul')
  })

  it('carries a section that is unavailable rather than merely empty', () => {
    // A happy-path-only fixture means the UI never once meets this distinction.
    expect(snapshot.projects.status).toBe('unavailable')
    expect(snapshot.projects.unavailable_reason).toBe('backend_unavailable')
    expect(snapshot.projects.items).toHaveLength(0)
  })

  it('never carries a message body — only a bounded preview', () => {
    for (const section of [snapshot.mail, snapshot.messenger]) {
      for (const thread of section.items) {
        for (const forbidden of ['body', 'payload_json', 'payload', 'content', 'full_text']) {
          expect(thread).not.toHaveProperty(forbidden)
        }
        expect(typeof thread.last_message_preview).toBe('string')
      }
    }
  })

  it('keeps external contacts off the tenant-org axis', () => {
    // Rendering an external contact as an org member asserts something false
    // about a real person.
    const tenant = snapshot.actor.organization_id
    for (const thread of snapshot.mail.items) {
      for (const participant of thread.participants) {
        if (participant.kind === 'external_counterparty_contact') {
          expect(participant.affiliation_id).not.toBe(tenant)
        }
      }
    }
  })
})

describe('failure classification (#9625)', () => {
  it('recognises each context-home code, and narrows to one when asked', () => {
    for (const code of Object.values(CONTEXT_HOME_ERROR_CODES)) {
      expect(isContextHomeError({ code, message: 'x' })).toBe(true)
    }

    const denied = { code: CONTEXT_HOME_ERROR_CODES.permissionDenied, message: 'x' }
    expect(isContextHomeError(denied, CONTEXT_HOME_ERROR_CODES.permissionDenied)).toBe(true)
    expect(isContextHomeError(denied, CONTEXT_HOME_ERROR_CODES.sessionExpired)).toBe(false)
  })

  it('keeps session-expiry and permission-denial separable', () => {
    // Folding these two together sends an unauthorized user to a login screen
    // where nothing resolves. That distinction is the point of this slice.
    expect(CONTEXT_HOME_ERROR_CODES.sessionExpired).not.toBe(CONTEXT_HOME_ERROR_CODES.permissionDenied)
  })

  it('does not offer retry for a permission denial or an expired session', () => {
    expect(isRetryableContextHomeError({ code: CONTEXT_HOME_ERROR_CODES.permissionDenied, message: 'x' })).toBe(false)
    expect(isRetryableContextHomeError({ code: CONTEXT_HOME_ERROR_CODES.sessionExpired, message: 'x' })).toBe(false)
    expect(isRetryableContextHomeError({ code: CONTEXT_HOME_ERROR_CODES.invalidResponse, message: 'x' })).toBe(false)

    for (const code of [CONTEXT_HOME_ERROR_CODES.timeout, CONTEXT_HOME_ERROR_CODES.unavailable]) {
      expect(isRetryableContextHomeError({ code, message: 'x' })).toBe(true)
    }
  })

  it('does not claim unrelated rejections', () => {
    expect(isContextHomeError({ code: 'handoff.rejected', message: 'x' })).toBe(false)
    expect(isContextHomeError(new Error('boom'))).toBe(false)
    expect(isContextHomeError('legacy string error')).toBe(false)
    expect(isContextHomeError(null)).toBe(false)
  })

  it('does not classify a missing bridge as a context-home failure', () => {
    // Nothing was refused and nothing failed — there is simply no desktop to ask.
    expect(isContextHomeError(new ContextHomeBridgeUnavailableError())).toBe(false)
    expect(isRetryableContextHomeError(new ContextHomeBridgeUnavailableError())).toBe(false)
  })
})
