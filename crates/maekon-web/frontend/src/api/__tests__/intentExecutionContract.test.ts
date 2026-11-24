import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiClientError, executeIntentHint, fetchIntentExecution } from '../client'
import { handleStandaloneRequest, isStandaloneModeEnabled } from '../standalone'
import { makeProvenance, makeResponse, PRIVATE_CANARY, SUBMISSION_A, SUBMISSION_B } from './fixtures/intentExecution'

vi.mock('../standalone', () => ({ isStandaloneModeEnabled: vi.fn(() => false), handleStandaloneRequest: vi.fn() }))
vi.mock('../../utils/api-base', () => ({
  resolveApiUrl: vi.fn(async (url: string) => `http://127.0.0.1:19090${url}`),
  resolveLocalAuthToken: vi.fn(async () => 'synthetic-auth'),
  withResolvedLocalAuthHeaders: vi.fn(async (init: RequestInit) => ({
    ...init,
    headers: new Headers({ ...(init.headers as Record<string, string>), 'X-Local-Auth': 'synthetic-auth' }),
  })),
}))

const request = {
  command_id: 'same-command',
  session_id: 'same-session',
  intent_hint: 'synthetic action',
  submission_id: SUBMISSION_A,
}
const jsonResponse = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
let fetchMock: ReturnType<typeof vi.fn>
beforeEach(() => {
  fetchMock = vi.fn()
  vi.stubGlobal('fetch', fetchMock)
  vi.mocked(isStandaloneModeEnabled).mockReturnValue(false)
  vi.mocked(handleStandaloneRequest).mockReset()
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe('dedicated observed intent transport', () => {
  it('sends the correlation and known retry link exactly once using local authentication', async () => {
    const p = makeProvenance()
    fetchMock.mockResolvedValue(jsonResponse(makeResponse(p)))
    const result = await executeIntentHint({ ...request, retry_of: p.execution_id })
    expect(fetchMock).toHaveBeenCalledOnce()
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('http://127.0.0.1:19090/api/automation/execute-hint')
    expect(init.method).toBe('POST')
    expect(new Headers(init.headers).get('X-Local-Auth')).toBe('synthetic-auth')
    expect(JSON.parse(init.body)).toEqual({ ...request, retry_of: p.execution_id })
    expect(result.execution_provenance).toEqual(p)
    expect(handleStandaloneRequest).not.toHaveBeenCalled()
  })

  it.each([
    400, 401, 500, 503,
  ])('preserves HTTP %s observations without automatic retry or demo fallback', async (status) => {
    const p = makeProvenance({ planning_outcome: 'denied', action_outcome: 'not_started' })
    fetchMock.mockResolvedValue(
      jsonResponse({ code: 'policy.denied', error: PRIVATE_CANARY, status, execution_provenance: p }, status),
    )
    const error = await executeIntentHint(request).catch((e: unknown) => e)
    expect(error).toBeInstanceOf(ApiClientError)
    expect(error).toMatchObject({ status, executionProvenance: p })
    expect(JSON.stringify((error as ApiClientError).executionProvenance)).not.toContain(PRIVATE_CANARY)
    expect(fetchMock).toHaveBeenCalledOnce()
    expect(handleStandaloneRequest).not.toHaveBeenCalled()
    expect(isStandaloneModeEnabled()).toBe(false)
  })

  it('does not expose mismatched or malicious provenance from successful or rejected responses', async () => {
    const wrong = makeProvenance({ submission_id: SUBMISSION_B })
    fetchMock.mockResolvedValueOnce(jsonResponse(makeResponse(wrong)))
    expect((await executeIntentHint(request)).execution_provenance).toBeNull()
    fetchMock.mockResolvedValueOnce(
      jsonResponse(
        { error: PRIVATE_CANARY, execution_provenance: { ...makeProvenance(), endpoint: PRIVATE_CANARY } },
        500,
      ),
    )
    const error = await executeIntentHint(request).catch((e: ApiClientError) => e)
    expect(error).toMatchObject({ executionProvenance: null })
  })

  it('keeps legacy live and explicitly selected demo responses usable without invented observations', async () => {
    const legacy = makeResponse(null)
    delete legacy.execution_provenance
    fetchMock.mockResolvedValueOnce(jsonResponse(legacy))
    expect(
      (await executeIntentHint({ session_id: 'legacy', intent_hint: 'synthetic' })).execution_provenance,
    ).toBeNull()
    vi.mocked(isStandaloneModeEnabled).mockReturnValue(true)
    vi.mocked(handleStandaloneRequest).mockResolvedValueOnce(jsonResponse(legacy))
    expect((await executeIntentHint(request)).execution_provenance).toBeNull()
    expect(fetchMock).toHaveBeenCalledOnce()
    expect(await fetchIntentExecution(SUBMISSION_A)).toBeNull()
    expect(fetchMock).toHaveBeenCalledOnce()
  })

  it('aborts a lost POST response at 30 seconds without issuing a second execution', async () => {
    vi.useFakeTimers()
    fetchMock.mockImplementation(
      (_url: string, init: RequestInit) =>
        new Promise((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => reject(new DOMException('Synthetic timeout', 'AbortError')))
        }),
    )
    const promise = executeIntentHint(request)
    const rejected = expect(promise).rejects.toMatchObject({ name: 'AbortError' })
    await vi.advanceTimersByTimeAsync(30_000)
    await rejected
    expect(fetchMock).toHaveBeenCalledOnce()
    expect(handleStandaloneRequest).not.toHaveBeenCalled()
  })
})

describe('exact observation GET', () => {
  it('uses only the retained submission ID, with auth, no cached response and no mutation', async () => {
    const p = makeProvenance()
    fetchMock.mockResolvedValue(
      jsonResponse([
        { command_id: 'same-command', session_id: 'same-session', details: PRIVATE_CANARY, execution_provenance: p },
      ]),
    )
    expect(await fetchIntentExecution(SUBMISSION_A)).toEqual(p)
    expect(fetchMock).toHaveBeenCalledOnce()
    const [url, init] = fetchMock.mock.calls[0]
    const parsed = new URL(url)
    expect(parsed.pathname).toBe('/api/automation/audit')
    expect([...parsed.searchParams.entries()]).toEqual([['submission_id', SUBMISSION_A]])
    expect(init.method).toBe('GET')
    expect(init.cache).toBe('no-store')
    expect(init.body).toBeUndefined()
    expect(new Headers(init.headers).get('X-Local-Auth')).toBe('synthetic-auth')
  })

  it.each([
    [],
    [{ execution_provenance: makeProvenance({ submission_id: SUBMISSION_B }) }],
    [{ execution_provenance: makeProvenance() }, { execution_provenance: makeProvenance() }],
    { entries: [] },
  ])('keeps absent, mismatched or ambiguous mappings unknown', async (body) => {
    fetchMock.mockResolvedValue(jsonResponse(body))
    expect(await fetchIntentExecution(SUBMISSION_A)).toBeNull()
    expect(fetchMock).toHaveBeenCalledOnce()
  })

  it.each([401, 403, 404, 429, 500, 503])('keeps HTTP %s unavailable without retry/fallback', async (status) => {
    fetchMock.mockResolvedValue(jsonResponse({ error: PRIVATE_CANARY }, status))
    expect(await fetchIntentExecution(SUBMISSION_A)).toBeNull()
    expect(fetchMock).toHaveBeenCalledOnce()
    expect(handleStandaloneRequest).not.toHaveBeenCalled()
  })

  it('keeps invalid IDs off the transport and malformed JSON unknown', async () => {
    expect(await fetchIntentExecution(PRIVATE_CANARY)).toBeNull()
    expect(fetchMock).not.toHaveBeenCalled()
    fetchMock.mockResolvedValue(new Response('{broken', { status: 200 }))
    expect(await fetchIntentExecution(SUBMISSION_A)).toBeNull()
    expect(fetchMock).toHaveBeenCalledOnce()
  })

  it('bounds lookup time without replaying the execution', async () => {
    vi.useFakeTimers()
    fetchMock.mockImplementation(
      (_url: string, init: RequestInit) =>
        new Promise((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => reject(new DOMException('Synthetic timeout', 'AbortError')))
        }),
    )
    const promise = fetchIntentExecution(SUBMISSION_A)
    await vi.advanceTimersByTimeAsync(10_000)
    expect(await promise).toBeNull()
    expect(fetchMock).toHaveBeenCalledOnce()
    expect(fetchMock.mock.calls[0][1].method).toBe('GET')
  })
})
