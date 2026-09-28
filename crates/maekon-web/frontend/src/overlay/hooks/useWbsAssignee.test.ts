import { act, cleanup, renderHook } from '@testing-library/react'
import { StrictMode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as api from '../../api/wbsAssignee'
import type { WbsConsentOffer, WbsExecution, WbsRecommendation, WbsSessionView, WbsTicket } from '../wbsTypes'
import { useWbsAssignee } from './useWbsAssignee'

vi.mock('../../api/wbsAssignee', () => ({
  offerWbsDocumentConsent: vi.fn(),
  openWbsSession: vi.fn(),
  recommendWbsAssignees: vi.fn(),
  confirmWbsCandidate: vi.fn(),
  executeWbsAssignee: vi.fn(),
  readWbsExecution: vi.fn(),
  cancelWbsSession: vi.fn(),
}))

type HookResult = { current: ReturnType<typeof useWbsAssignee> }
const expiresIn = (ms = 60_000) => new Date(Date.now() + ms).toISOString()
const authorization = { session_id: 'session', capability: 'a'.repeat(64) }
const makeOffer = (overrides: Partial<WbsConsentOffer> = {}): WbsConsentOffer => ({
  offer_id: 'offer',
  nonce: 'b'.repeat(64),
  scope_id: 'scope',
  notice_version: 'wbs-cell-assignee.v1',
  document_display_label: 'Synthetic demo',
  permitted_operations: ['read_context', 'fetch_candidates', 'apply_one_literal_assignee'],
  expires_at: expiresIn(),
  ...overrides,
})
const makeSession = (overrides: Partial<WbsSessionView> = {}): WbsSessionView => ({
  authorization,
  scope_id: 'scope',
  expires_at: expiresIn(),
  ...overrides,
})
const makeRecommendation = (overrides: Partial<WbsRecommendation> = {}): WbsRecommendation => ({
  session_id: authorization.session_id,
  query_generation: '1',
  context: {
    source_kind: 'local_document_roster',
    organization_id: null,
    wbs_item_id: 'wbs',
    wbs_version_id: null,
    local: {
      document_registration_id: 'synthetic-document',
      input_revision: 'input-1',
      input_hash: '1'.repeat(64),
      roster_revision: 'roster-1',
      roster_hash: '2'.repeat(64),
    },
  },
  source: {
    source_kind: 'local_document_roster',
    snapshot_id: 'synthetic-snapshot',
    snapshot_version: '1',
    snapshot_hash: '3'.repeat(64),
    wbs_version_id: null,
    wbs_content_hash: null,
    approval_id: null,
    local: {
      input: {
        document_registration_id: 'synthetic-document',
        input_revision: 'input-1',
        input_hash: '1'.repeat(64),
        roster_revision: 'roster-1',
        roster_hash: '2'.repeat(64),
      },
      provider_selection_digest: '4'.repeat(64),
      provider_name: 'Synthetic provider',
      model: 'synthetic-model',
      prompt_revision: 'prompt-1',
    },
  },
  synthetic: true,
  provenance: ['synthetic-fixture'],
  target_hint: { row: 5, column: 2 },
  candidates: [
    {
      candidate_id: 'candidate',
      candidate_proof: 'c'.repeat(64),
      display_name: 'Alex',
      rank: 1,
      eligible: true,
      rank_reason: 'Synthetic roster match',
    },
  ],
  expires_at: expiresIn(),
  ...overrides,
})
const makeTicket = (overrides: Partial<WbsTicket> = {}): WbsTicket => ({
  operation_id: 'operation',
  session_id: authorization.session_id,
  nonce: 'd'.repeat(64),
  payload_digest: 'e'.repeat(64),
  expires_at: expiresIn(),
  signature: 'f'.repeat(64),
  ...overrides,
})
const makeExecution = (overrides: Partial<WbsExecution> = {}): WbsExecution => ({
  operation_id: 'operation',
  disposition: 'applied_verified',
  native_outcome: 'written_unverified',
  readback: 'matches',
  audit: 'completion_accepted',
  ...overrides,
})
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: Error) => void
  const promise = new Promise<T>((onResolve, onReject) => {
    resolve = onResolve
    reject = onReject
  })
  return { promise, resolve, reject }
}
async function ready(
  result: HookResult,
  opened = makeSession(),
  view = makeRecommendation({ session_id: opened.authorization.session_id }),
) {
  vi.mocked(api.offerWbsDocumentConsent).mockResolvedValueOnce(makeOffer())
  vi.mocked(api.openWbsSession).mockResolvedValueOnce(opened)
  vi.mocked(api.recommendWbsAssignees).mockResolvedValueOnce(view)
  await act(async () => {
    await result.current.requestConsent()
  })
  await act(async () => {
    await result.current.acceptConsent()
  })
  return { opened, view, candidate: view.candidates[0] }
}
function mockExecution(result = makeExecution()) {
  vi.mocked(api.confirmWbsCandidate).mockResolvedValue(makeTicket())
  vi.mocked(api.executeWbsAssignee).mockResolvedValue(result)
  vi.mocked(api.readWbsExecution).mockResolvedValue(result)
}
beforeEach(() => {
  vi.resetAllMocks()
  vi.useFakeTimers()
  vi.setSystemTime(new Date('2026-09-07T00:00:00.000Z'))
  vi.mocked(api.cancelWbsSession).mockImplementation(async (auth) => ({
    session_id: auth.session_id,
    closed: true,
    operations: [],
  }))
})
afterEach(() => {
  cleanup()
  vi.clearAllTimers()
  vi.useRealTimers()
})

describe('useWbsAssignee lifetimes', () => {
  const gateCodes = [
    'wbs_consent_required',
    'wbs_consent_changed',
    'wbs_policy_blocked',
    'wbs_sandbox_unsupported',
    'wbs_configuration_changed',
    'wbs_expired',
  ]
  it.each(gateCodes)('preserves a pre-execution offer rejection: %s', async (code) => {
    vi.mocked(api.offerWbsDocumentConsent).mockRejectedValueOnce({ code, message: 'Private native diagnostic' })
    const { result } = renderHook(() => useWbsAssignee())
    await act(async () => {
      await result.current.requestConsent()
      await result.current.acceptConsent()
    })
    expect(result.current).toMatchObject({ status: 'unavailable', error: code, offer: null, execution: null })
    expect(api.openWbsSession).not.toHaveBeenCalled()
    expect(api.recommendWbsAssignees).not.toHaveBeenCalled()
    expect(api.confirmWbsCandidate).not.toHaveBeenCalled()
    expect(api.executeWbsAssignee).not.toHaveBeenCalled()
  })
  it.each(['opening', 'recommendation', 'refresh'])('preserves a consent rejection while %s', async (phase) => {
    const failure = { code: 'wbs_consent_changed', message: 'Private native diagnostic' }
    const { result } = renderHook(() => useWbsAssignee())
    if (phase === 'refresh') {
      await ready(result)
      vi.mocked(api.recommendWbsAssignees).mockRejectedValueOnce(failure)
      await act(async () => {
        await result.current.refresh()
      })
    } else {
      vi.mocked(api.offerWbsDocumentConsent).mockResolvedValueOnce(makeOffer())
      if (phase === 'opening') vi.mocked(api.openWbsSession).mockRejectedValueOnce(failure)
      else {
        vi.mocked(api.openWbsSession).mockResolvedValueOnce(makeSession())
        vi.mocked(api.recommendWbsAssignees).mockRejectedValueOnce(failure)
      }
      await act(async () => {
        await result.current.requestConsent()
        await result.current.acceptConsent()
      })
    }
    expect(result.current).toMatchObject({ status: 'unavailable', error: 'wbs_consent_changed', recommendation: null })
    expect(api.confirmWbsCandidate).not.toHaveBeenCalled()
    expect(api.executeWbsAssignee).not.toHaveBeenCalled()
    if (phase === 'opening') expect(api.recommendWbsAssignees).not.toHaveBeenCalled()
  })
  it.each(gateCodes)('reports confirmation rejection before issuing any write: %s', async (code) => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    vi.mocked(api.confirmWbsCandidate).mockRejectedValueOnce({ code, message: 'Private native diagnostic' })
    await act(async () => {
      await result.current.choose(candidate)
      await result.current.checkExecution()
    })
    expect(result.current).toMatchObject({ status: 'unavailable', error: code, recommendation: null, execution: null })
    expect(api.confirmWbsCandidate).toHaveBeenCalledTimes(1)
    expect(api.executeWbsAssignee).not.toHaveBeenCalled()
    expect(api.readWbsExecution).not.toHaveBeenCalled()
  })
  it.each(gateCodes)('keeps a dispatched write uncertain even for a known rejection code: %s', async (code) => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    vi.mocked(api.confirmWbsCandidate).mockResolvedValueOnce(makeTicket())
    vi.mocked(api.executeWbsAssignee).mockRejectedValueOnce({ code, message: 'Private native diagnostic' })
    await act(async () => {
      await result.current.choose(candidate)
      await result.current.choose(candidate)
    })
    expect(result.current).toMatchObject({
      status: 'ready',
      error: 'wbs_outcome_unknown',
      execution: {
        operation_id: 'operation',
        disposition: 'outcome_unknown',
        native_outcome: 'outcome_unknown',
        readback: 'not_read',
        audit: 'not_started',
      },
    })
    expect(api.confirmWbsCandidate).toHaveBeenCalledTimes(1)
    expect(api.executeWbsAssignee).toHaveBeenCalledTimes(1)
    expect(api.readWbsExecution).not.toHaveBeenCalled()
  })
  it.each([
    'automation was disabled',
    'the native message changed',
  ])('preserves the typed disabled code from offer failures: %s', async (message) => {
    vi.mocked(api.offerWbsDocumentConsent).mockRejectedValueOnce({ code: 'wbs_automation_disabled', message })
    const { result } = renderHook(() => useWbsAssignee())
    await act(async () => {
      await result.current.requestConsent()
    })
    expect(result.current).toMatchObject({ status: 'unavailable', error: 'wbs_automation_disabled' })
    expect(api.openWbsSession).not.toHaveBeenCalled()
    expect(api.recommendWbsAssignees).not.toHaveBeenCalled()
    expect(api.executeWbsAssignee).not.toHaveBeenCalled()
  })
  it.each<unknown>([
    { code: 'wbs_other_failure', message: 'disabled words must not matter' },
    { code: '__proto__', message: 'wbs_consent_required' },
    { code: 'wbs_consent_required ', message: 'wbs_consent_required' },
    { code: 'wbs_consent_required' },
    { code: 'wbs_policy_blocked', message: 42 },
    'wbs_automation_disabled',
    { code: 'wbs_automation_disabled', message: 42 },
    new Error('wbs_automation_disabled'),
  ])('keeps unknown or malformed offer failures generic: %j', async (failure) => {
    vi.mocked(api.offerWbsDocumentConsent).mockRejectedValueOnce(failure)
    const { result } = renderHook(() => useWbsAssignee())
    await act(async () => {
      await result.current.requestConsent()
    })
    expect(result.current).toMatchObject({ status: 'unavailable', error: 'wbs_unavailable' })
    expect(api.openWbsSession).not.toHaveBeenCalled()
  })
  it('preserves typed disabled guidance from a live recommendation failure without execution', async () => {
    vi.mocked(api.offerWbsDocumentConsent).mockResolvedValueOnce(makeOffer())
    vi.mocked(api.openWbsSession).mockResolvedValueOnce(makeSession())
    vi.mocked(api.recommendWbsAssignees).mockRejectedValueOnce({
      code: 'wbs_automation_disabled',
      message: 'live toggle state',
    })
    const { result } = renderHook(() => useWbsAssignee())
    await act(async () => {
      await result.current.requestConsent()
      await result.current.acceptConsent()
    })
    expect(result.current).toMatchObject({ status: 'unavailable', error: 'wbs_automation_disabled' })
    expect(api.confirmWbsCandidate).not.toHaveBeenCalled()
    expect(api.executeWbsAssignee).not.toHaveBeenCalled()
  })
  it('keeps cancellation final when a late disabled offer failure arrives', async () => {
    const pending = deferred<WbsConsentOffer>()
    vi.mocked(api.offerWbsDocumentConsent).mockReturnValueOnce(pending.promise)
    const { result } = renderHook(() => useWbsAssignee())
    let request: Promise<void> | undefined
    act(() => {
      request = result.current.requestConsent()
    })
    await act(async () => {
      await result.current.cancel()
    })
    await act(async () => {
      pending.reject({ code: 'wbs_automation_disabled', message: 'late' })
      await request
    })
    expect(result.current).toMatchObject({ status: 'cancelled', error: null })
    expect(api.openWbsSession).not.toHaveBeenCalled()
    expect(api.executeWbsAssignee).not.toHaveBeenCalled()
  })
  it('starts inert and makes no API calls on an empty cancellation', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    expect(result.current.status).toBe('idle')
    expect(api.offerWbsDocumentConsent).not.toHaveBeenCalled()
    await act(async () => {
      await result.current.cancel()
    })
    expect(result.current.status).toBe('cancelled')
    expect(api.cancelWbsSession).not.toHaveBeenCalled()
  })
  it('ignores an old offer arriving after cancellation', async () => {
    const pending = deferred<WbsConsentOffer>()
    vi.mocked(api.offerWbsDocumentConsent).mockReturnValueOnce(pending.promise)
    const { result } = renderHook(() => useWbsAssignee())
    let request: Promise<void> | undefined
    act(() => {
      request = result.current.requestConsent()
    })
    await act(async () => {
      await result.current.cancel()
    })
    await act(async () => {
      pending.resolve(makeOffer())
      await request
    })
    expect(result.current.status).toBe('cancelled')
    expect(result.current.offer).toBeNull()
  })
  it('disposes a session arriving after unmount and handles rejected cancellation', async () => {
    const pending = deferred<WbsSessionView>()
    vi.mocked(api.offerWbsDocumentConsent).mockResolvedValueOnce(makeOffer())
    vi.mocked(api.openWbsSession).mockReturnValueOnce(pending.promise)
    vi.mocked(api.cancelWbsSession).mockRejectedValueOnce(new Error('transport closed'))
    const { result, unmount } = renderHook(() => useWbsAssignee())
    await act(async () => {
      await result.current.requestConsent()
    })
    let opening: Promise<void> | undefined
    act(() => {
      opening = result.current.acceptConsent()
    })
    unmount()
    const opened = makeSession()
    await act(async () => {
      pending.resolve(opened)
      await opening
    })
    expect(api.cancelWbsSession).toHaveBeenCalledExactlyOnceWith(opened.authorization)
    expect(api.recommendWbsAssignees).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
  })
  it('handles synchronous cancellation failure during unmount', async () => {
    const { result, unmount } = renderHook(() => useWbsAssignee())
    await ready(result)
    vi.mocked(api.cancelWbsSession).mockImplementationOnce(() => {
      throw new Error('bridge closed')
    })
    expect(() => unmount()).not.toThrow()
    await act(async () => {
      await Promise.resolve()
    })
    expect(vi.getTimerCount()).toBe(0)
  })
  it('survives StrictMode setup and rerenders without cancelling the session', async () => {
    const { result, rerender } = renderHook(() => useWbsAssignee(), { wrapper: StrictMode })
    await ready(result)
    const callbacks = [result.current.cancel, result.current.refresh, result.current.checkExecution]
    rerender()
    expect(result.current.status).toBe('ready')
    expect(api.cancelWbsSession).not.toHaveBeenCalled()
    expect([result.current.cancel, result.current.refresh, result.current.checkExecution]).toEqual(callbacks)
    expect(vi.getTimerCount()).toBe(1)
  })
  it('cancels while confirmation is pending without executing', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    const pending = deferred<WbsTicket>()
    vi.mocked(api.confirmWbsCandidate).mockReturnValueOnce(pending.promise)
    let choosing: Promise<void> | undefined
    act(() => {
      choosing = result.current.choose(candidate)
    })
    await act(async () => {
      await result.current.cancel()
    })
    await act(async () => {
      pending.resolve(makeTicket())
      await choosing
    })
    await act(async () => {
      await result.current.checkExecution()
    })
    expect(api.executeWbsAssignee).not.toHaveBeenCalled()
    expect(api.readWbsExecution).not.toHaveBeenCalled()
    expect(result.current.execution).toBeNull()
    expect(result.current.recommendation).toBeNull()
    expect(result.current.status).toBe('cancelled')
  })
  it('rejects an expired offer without opening a session', async () => {
    vi.mocked(api.offerWbsDocumentConsent).mockResolvedValueOnce(makeOffer({ expires_at: expiresIn(-1) }))
    const { result } = renderHook(() => useWbsAssignee())
    await act(async () => {
      await result.current.requestConsent()
      await result.current.acceptConsent()
    })
    expect(api.openWbsSession).not.toHaveBeenCalled()
    expect(result.current.offer).toBeNull()
    expect(result.current.error).toBe('wbs_expired')
  })
  it.each([
    { expires_at: '2026-09-06T23:59:59.000Z' },
    { scope_id: 'another-document' },
  ])('rejects a stale or wrong-scope opened session: %o', async (overrides) => {
    const { result } = renderHook(() => useWbsAssignee())
    const opened = makeSession(overrides)
    await ready(result, opened)
    expect(api.recommendWbsAssignees).not.toHaveBeenCalled()
    expect(api.cancelWbsSession).toHaveBeenCalledWith(opened.authorization)
    expect(result.current.recommendation).toBeNull()
    expect(result.current.status).toBe('unavailable')
  })
  it.each([
    { expires_at: '2026-09-06T23:59:59.000Z' },
    { session_id: 'another-session' },
  ])('rejects a stale or wrong-session recommendation: %o', async (overrides) => {
    const { result } = renderHook(() => useWbsAssignee())
    const view = makeRecommendation(overrides)
    await ready(result, makeSession(), view)
    await act(async () => {
      await result.current.choose(view.candidates[0])
    })
    expect(result.current.recommendation).toBeNull()
    expect(api.confirmWbsCandidate).not.toHaveBeenCalled()
    expect(result.current.status).toBe('unavailable')
  })
  it.each([
    { expires_at: '2026-09-06T23:59:59.000Z' },
    { session_id: 'another-session' },
  ])('rejects a stale or wrong-session ticket before execution: %o', async (overrides) => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    vi.mocked(api.confirmWbsCandidate).mockResolvedValueOnce(makeTicket(overrides))
    await act(async () => {
      await result.current.choose(candidate)
      await result.current.checkExecution()
    })
    expect(api.executeWbsAssignee).not.toHaveBeenCalled()
    expect(api.readWbsExecution).not.toHaveBeenCalled()
    expect(result.current.execution).toBeNull()
    expect(result.current.recommendation).toBeNull()
  })
  it('rechecks session expiry after confirmation even before the timer runs', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result, makeSession({ expires_at: expiresIn(1_000) }))
    const pending = deferred<WbsTicket>()
    vi.mocked(api.confirmWbsCandidate).mockReturnValueOnce(pending.promise)
    let choosing: Promise<void> | undefined
    act(() => {
      choosing = result.current.choose(candidate)
    })
    vi.setSystemTime(new Date(Date.now() + 1_001))
    await act(async () => {
      pending.resolve(makeTicket())
      await choosing
    })
    expect(api.executeWbsAssignee).not.toHaveBeenCalled()
    expect(result.current.recommendation).toBeNull()
    expect(result.current.error).toBe('wbs_expired')
  })
  it('ignores recommendations arriving after session expiry while loading', async () => {
    const pending = deferred<WbsRecommendation>()
    vi.mocked(api.offerWbsDocumentConsent).mockResolvedValueOnce(makeOffer())
    vi.mocked(api.openWbsSession).mockResolvedValueOnce(makeSession({ expires_at: expiresIn(1_000) }))
    vi.mocked(api.recommendWbsAssignees).mockReturnValueOnce(pending.promise)
    const { result } = renderHook(() => useWbsAssignee())
    await act(async () => {
      await result.current.requestConsent()
    })
    let opening: Promise<void> | undefined
    await act(async () => {
      opening = result.current.acceptConsent()
      await Promise.resolve()
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_001)
    })
    await act(async () => {
      pending.resolve(makeRecommendation())
      await opening
    })
    expect(result.current.status).toBe('cancelled')
    expect(result.current.recommendation).toBeNull()
    expect(api.cancelWbsSession).toHaveBeenCalledExactlyOnceWith(authorization)
  })
  it('expires a recommendation before its longer lived session and removes candidates', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result, makeSession(), makeRecommendation({ expires_at: expiresIn(1_000) }))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_001)
    })
    await act(async () => {
      await result.current.choose(candidate)
    })
    expect(result.current.recommendation).toBeNull()
    expect(api.confirmWbsCandidate).not.toHaveBeenCalled()
    expect(result.current.error).toBe('wbs_expired')
  })
  it('refresh clears success and invalidates old candidates and saved callbacks', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    mockExecution()
    await act(async () => {
      await result.current.choose(candidate)
    })
    expect(result.current.execution?.disposition).toBe('applied_verified')
    const oldChoose = result.current.choose
    const pending = deferred<WbsRecommendation>()
    vi.mocked(api.recommendWbsAssignees).mockReturnValueOnce(pending.promise)
    let refreshing: Promise<void> | undefined
    act(() => {
      refreshing = result.current.refresh()
    })
    expect(result.current.execution).toBeNull()
    expect(result.current.recommendation).toBeNull()
    const next = makeRecommendation({ query_generation: '2' })
    await act(async () => {
      pending.resolve(next)
      await refreshing
    })
    await act(async () => {
      await oldChoose(candidate)
      await result.current.choose(candidate)
      await result.current.checkExecution()
    })
    expect(api.confirmWbsCandidate).toHaveBeenCalledTimes(1)
    expect(api.readWbsExecution).toHaveBeenCalledTimes(1)
    expect(result.current.recommendation).toBe(next)
    await act(async () => {
      await result.current.choose(next.candidates[0])
    })
    expect(api.confirmWbsCandidate).toHaveBeenLastCalledWith(authorization, {
      query_generation: '2',
      candidate_id: next.candidates[0].candidate_id,
      candidate_proof: next.candidates[0].candidate_proof,
    })
  })
  it('does not revive an old candidate after failed refresh', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    const oldChoose = result.current.choose
    vi.mocked(api.recommendWbsAssignees).mockRejectedValueOnce(new Error('unavailable'))
    await act(async () => {
      await result.current.refresh()
    })
    await act(async () => {
      await oldChoose(candidate)
      await result.current.choose(candidate)
    })
    expect(result.current.recommendation).toBeNull()
    expect(api.confirmWbsCandidate).not.toHaveBeenCalled()
  })
  it('rejects cloned candidates and duplicate clicks during and after execution', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    mockExecution(
      makeExecution({
        disposition: 'outcome_unknown',
        native_outcome: 'outcome_unknown',
        audit: 'completion_rejected',
      }),
    )
    await act(async () => {
      await result.current.choose({ ...candidate })
    })
    expect(api.confirmWbsCandidate).not.toHaveBeenCalled()
    await act(async () => {
      await Promise.all([result.current.choose(candidate), result.current.choose(candidate)])
    })
    await act(async () => {
      await result.current.choose(candidate)
    })
    expect(api.executeWbsAssignee).toHaveBeenCalledTimes(1)
    expect(result.current.execution?.disposition).toBe('outcome_unknown')
    expect(result.current.execution?.readback).toBe('matches')
  })
  it('does not invent an operation when confirmation fails', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    vi.mocked(api.confirmWbsCandidate).mockRejectedValueOnce(new Error('not confirmed'))
    await act(async () => {
      await result.current.choose(candidate)
      await result.current.checkExecution()
    })
    expect(result.current.execution).toBeNull()
    expect(result.current.error).toBe('wbs_unavailable')
    expect(api.executeWbsAssignee).not.toHaveBeenCalled()
    expect(api.readWbsExecution).not.toHaveBeenCalled()
  })
  it('does not reuse a previous operation on confirmation failure after refresh', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    mockExecution()
    await act(async () => {
      await result.current.choose(candidate)
    })
    const next = makeRecommendation({ query_generation: '2' })
    vi.mocked(api.recommendWbsAssignees).mockResolvedValueOnce(next)
    await act(async () => {
      await result.current.refresh()
    })
    vi.mocked(api.confirmWbsCandidate).mockRejectedValueOnce(new Error('not confirmed'))
    await act(async () => {
      await result.current.choose(next.candidates[0])
      await result.current.checkExecution()
    })
    expect(result.current.execution).toBeNull()
    expect(api.executeWbsAssignee).toHaveBeenCalledTimes(1)
    expect(api.readWbsExecution).toHaveBeenCalledTimes(1)
  })
  it('keeps a known operation unknown after execute failure and only retries its read', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    vi.mocked(api.confirmWbsCandidate).mockResolvedValueOnce(makeTicket())
    vi.mocked(api.executeWbsAssignee).mockRejectedValueOnce(new Error('lost acknowledgement'))
    await act(async () => {
      await result.current.choose(candidate)
    })
    expect(result.current.execution).toMatchObject({ operation_id: 'operation', disposition: 'outcome_unknown' })
    vi.mocked(api.readWbsExecution)
      .mockRejectedValueOnce(new Error('still unavailable'))
      .mockResolvedValueOnce(makeExecution())
    await act(async () => {
      await result.current.checkExecution()
      await result.current.choose(candidate)
    })
    expect(result.current.execution?.disposition).toBe('outcome_unknown')
    await act(async () => {
      await result.current.checkExecution()
    })
    expect(api.executeWbsAssignee).toHaveBeenCalledTimes(1)
    expect(vi.mocked(api.readWbsExecution).mock.calls).toEqual([
      [authorization, 'operation'],
      [authorization, 'operation'],
    ])
    expect(result.current.execution?.disposition).toBe('applied_verified')
  })
  it('preserves the execution receipt when readback fails and recovers by the same operation', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    const receipt = makeExecution({
      disposition: 'outcome_unknown',
      native_outcome: 'outcome_unknown',
      readback: 'not_read',
      audit: 'start_accepted',
    })
    mockExecution(receipt)
    vi.mocked(api.readWbsExecution).mockRejectedValueOnce(new Error('lost read')).mockResolvedValueOnce(makeExecution())
    await act(async () => {
      await result.current.choose(candidate)
    })
    expect(result.current.execution).toBe(receipt)
    expect(result.current.error).toBe('wbs_readback_unavailable')
    await act(async () => {
      await result.current.checkExecution()
    })
    expect(api.executeWbsAssignee).toHaveBeenCalledTimes(1)
    expect(vi.mocked(api.readWbsExecution).mock.calls).toEqual([
      [authorization, 'operation'],
      [authorization, 'operation'],
    ])
    expect(result.current.execution?.disposition).toBe('applied_verified')
  })
  it('never follows an unrelated operation ID returned by execute', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    mockExecution(makeExecution({ operation_id: 'foreign-operation' }))
    await act(async () => {
      await result.current.choose(candidate)
    })
    expect(result.current.execution).toMatchObject({ operation_id: 'operation', disposition: 'outcome_unknown' })
    expect(api.readWbsExecution).not.toHaveBeenCalled()
    vi.mocked(api.readWbsExecution).mockResolvedValueOnce(makeExecution())
    await act(async () => {
      await result.current.checkExecution()
    })
    expect(api.readWbsExecution).toHaveBeenCalledExactlyOnceWith(authorization, 'operation')
    expect(api.executeWbsAssignee).toHaveBeenCalledTimes(1)
  })
  it('rejects unrelated automatic and manual read receipts without replacing known results', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const { candidate } = await ready(result)
    const receipt = makeExecution({ disposition: 'outcome_unknown' })
    mockExecution(receipt)
    vi.mocked(api.readWbsExecution).mockResolvedValue(makeExecution({ operation_id: 'foreign-operation' }))
    await act(async () => {
      await result.current.choose(candidate)
    })
    expect(result.current.execution).toBe(receipt)
    await act(async () => {
      await result.current.checkExecution()
    })
    expect(result.current.execution).toBe(receipt)
    expect(result.current.error).toBe('wbs_readback_unavailable')
    expect(api.executeWbsAssignee).toHaveBeenCalledTimes(1)
    expect(vi.mocked(api.readWbsExecution).mock.calls).toEqual([
      [authorization, 'operation'],
      [authorization, 'operation'],
    ])
  })
  it('restart disposes the prior session and expiry timer without retaining its operation', async () => {
    const { result } = renderHook(() => useWbsAssignee())
    const first = makeSession({ expires_at: expiresIn(1_000) })
    const { candidate } = await ready(result, first)
    mockExecution()
    await act(async () => {
      await result.current.choose(candidate)
    })
    const second = makeSession({ authorization: { session_id: 'session-2', capability: '2'.repeat(64) } })
    await ready(result, second)
    await act(async () => {
      await result.current.checkExecution()
    })
    expect(result.current.execution).toBeNull()
    expect(api.readWbsExecution).toHaveBeenCalledTimes(1)
    expect(api.cancelWbsSession).toHaveBeenCalledExactlyOnceWith(first.authorization)
    expect(vi.getTimerCount()).toBe(1)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_001)
    })
    expect(result.current.status).toBe('ready')
    expect(result.current.recommendation?.session_id).toBe('session-2')
    expect(api.cancelWbsSession).toHaveBeenCalledTimes(1)
    await act(async () => {
      await result.current.cancel()
      await result.current.checkExecution()
    })
    expect(api.cancelWbsSession).toHaveBeenLastCalledWith(second.authorization)
    expect(api.readWbsExecution).toHaveBeenCalledTimes(1)
    expect(vi.getTimerCount()).toBe(0)
  })
})
