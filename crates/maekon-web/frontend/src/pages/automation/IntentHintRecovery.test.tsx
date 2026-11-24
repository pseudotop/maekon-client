import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import {
  EXECUTION_A,
  EXECUTION_B,
  makeFallback,
  makeProvenance,
  makeResponse,
  opaqueId,
  PRIVATE_CANARY,
  SUBMISSION_A,
  SUBMISSION_B,
} from '../../api/__tests__/fixtures/intentExecution'
import {
  executeIntentHint,
  fetchIntentExecution,
  type IntentActionOutcome,
  type IntentExecutionProvenance,
} from '../../api/client'
import IntentHintBar from './IntentHintBar'

vi.mock('../../api/client', () => ({ executeIntentHint: vi.fn(), fetchIntentExecution: vi.fn() }))
const status = { enabled: true, llm_source: 'remote_model', llm_fallback_reason: null } as never
const execute = vi.mocked(executeIntentHint)
const lookup = vi.mocked(fetchIntentExecution)
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((res) => {
    resolve = res
  })
  return { promise, resolve }
}
async function submit(hint = 'synthetic action') {
  await act(async () => {
    fireEvent.change(screen.getByTestId('intent-hint-input'), { target: { value: hint } })
    fireEvent.submit(screen.getByTestId('intent-hint-input').closest('form')!)
  })
}
const settled = () => screen.findByTestId('intent-observe-current')
async function observe() {
  await act(async () => {
    fireEvent.click(screen.getByTestId('intent-observe-current'))
  })
}
const failure = (p?: IntentExecutionProvenance) => Object.assign(new Error(PRIVATE_CANARY), { executionProvenance: p })

beforeEach(() => {
  execute.mockReset()
  lookup.mockReset()
  let id = 0
  vi.stubGlobal('crypto', { randomUUID: vi.fn(() => opaqueId(++id)) })
})
afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('intent submission identity and recovery', () => {
  it('assigns identity before dispatch and prevents two synchronous submissions', async () => {
    const response = deferred<ReturnType<typeof makeResponse>>()
    execute.mockReturnValue(response.promise)
    renderWithProviders(<IntentHintBar status={status} />)
    fireEvent.change(screen.getByTestId('intent-hint-input'), { target: { value: 'synthetic action' } })
    const form = screen.getByTestId('intent-hint-input').closest('form')!
    await act(async () => {
      fireEvent.submit(form)
      fireEvent.submit(form)
    })
    expect(execute).toHaveBeenCalledOnce()
    expect(execute.mock.calls[0][0].submission_id).toBe(SUBMISSION_A)
    expect(vi.mocked(crypto.randomUUID).mock.invocationCallOrder[0]).toBeLessThan(execute.mock.invocationCallOrder[0])
    expect(screen.getByTestId('intent-hint-run')).toBeDisabled()
    expect(lookup).not.toHaveBeenCalled()
    await act(async () => {
      response.resolve(makeResponse())
    })
    await settled()
    expect(screen.getByTestId('intent-hint-run')).toBeEnabled()
  })

  it('retains a lost response identity and observes it without sending another action', async () => {
    execute.mockRejectedValue(new DOMException('Synthetic timeout', 'AbortError'))
    lookup.mockResolvedValue(makeFallback())
    renderWithProviders(<IntentHintBar status={status} />)
    await submit()
    await settled()
    expect(screen.getByText(/may still have executed/)).toBeInTheDocument()
    expect(screen.getByTestId('intent-provenance-unknown')).toBeInTheDocument()
    expect(screen.queryByText('Not started', { exact: true })).not.toBeInTheDocument()
    expect(screen.queryByText(/cancelled/i)).not.toBeInTheDocument()
    expect(lookup).not.toHaveBeenCalled()
    await observe()
    await waitFor(() => expect(screen.getByTestId('intent-observed-plan')).toHaveTextContent('Rule-based plan'))
    expect(lookup).toHaveBeenCalledExactlyOnceWith(SUBMISSION_A)
    expect(execute).toHaveBeenCalledOnce()
    expect(screen.getByText('Matching execution observed. No action was sent.')).toBeInTheDocument()
  })

  it('does not attach a late lookup for A after explicit execution B starts', async () => {
    const late = deferred<IntentExecutionProvenance | null>()
    execute
      .mockRejectedValueOnce(failure())
      .mockResolvedValueOnce(makeResponse(makeProvenance({ submission_id: SUBMISSION_B, execution_id: EXECUTION_B })))
    lookup.mockReturnValue(late.promise)
    renderWithProviders(<IntentHintBar status={status} />)
    await submit('first')
    await settled()
    await observe()
    await submit('second')
    await settled()
    expect(screen.getByTestId('intent-observed-plan')).toHaveTextContent('Model-based plan')
    await act(async () => {
      late.resolve(makeFallback())
    })
    expect(screen.getByTestId('intent-observed-plan')).toHaveTextContent('Model-based plan')
    expect(screen.queryByText('Matching execution observed. No action was sent.')).not.toBeInTheDocument()
    expect(execute).toHaveBeenCalledTimes(2)
    expect(execute.mock.calls[1][0]).toMatchObject({ submission_id: SUBMISSION_B, intent_hint: 'second' })
    expect(execute.mock.calls[1][0]).not.toHaveProperty('retry_of')
    expect(lookup).toHaveBeenCalledExactlyOnceWith(SUBMISSION_A)
  })

  it('does not replace a known execution with another execution sharing its submission', async () => {
    execute.mockResolvedValue(makeResponse())
    lookup.mockResolvedValue({
      ...makeFallback(),
      execution_id: EXECUTION_B,
      events: makeFallback().events.map((event) => ({ ...event, execution_id: EXECUTION_B })),
    })
    renderWithProviders(<IntentHintBar status={status} />)
    await submit()
    await settled()
    await observe()
    await waitFor(() => expect(screen.getByText(/No unique matching observation/)).toBeInTheDocument())
    expect(screen.getByTestId('intent-observed-plan')).toHaveTextContent('Model-based plan')
    expect(execute).toHaveBeenCalledOnce()
  })

  it.each([
    'completed',
    'failed',
    'denied',
    'not_started',
  ] as IntentActionOutcome[])('links an explicit same-hint retry only to the known %s execution', async (action) => {
    const known = makeProvenance({ action_outcome: action })
    if (action === 'completed') execute.mockResolvedValueOnce(makeResponse(known))
    else execute.mockRejectedValueOnce(failure(known))
    execute.mockResolvedValueOnce(
      makeResponse(makeProvenance({ submission_id: SUBMISSION_B, execution_id: EXECUTION_B, retry_of: EXECUTION_A })),
    )
    renderWithProviders(<IntentHintBar status={status} />)
    await submit()
    await settled()
    await submit()
    await settled()
    expect(execute).toHaveBeenCalledTimes(2)
    expect(execute.mock.calls[0][0]).not.toHaveProperty('retry_of')
    expect(execute.mock.calls[1][0]).toMatchObject({ submission_id: SUBMISSION_B, retry_of: EXECUTION_A })
    expect(execute.mock.calls[1][0].session_id).toBe(execute.mock.calls[0][0].session_id)
    expect(lookup).not.toHaveBeenCalled()
  })

  it.each([
    undefined,
    makeProvenance({ observation: 'partial' }),
    makeProvenance({ action_outcome: 'unknown' }),
  ])('does not infer a retry link from an unknown or partial prior result', async (prior) => {
    execute.mockRejectedValueOnce(failure(prior)).mockResolvedValueOnce(makeResponse(null))
    renderWithProviders(<IntentHintBar status={status} />)
    await submit()
    await settled()
    await submit()
    await settled()
    expect(execute.mock.calls[1][0].submission_id).toBe(SUBMISSION_B)
    expect(execute.mock.calls[1][0]).not.toHaveProperty('retry_of')
  })

  it('does not label a different explicit hint as a retry of the last completed action', async () => {
    execute.mockResolvedValueOnce(makeResponse()).mockResolvedValueOnce(makeResponse(null))
    renderWithProviders(<IntentHintBar status={status} />)
    await submit('first')
    await settled()
    await submit('unrelated')
    await settled()
    expect(execute.mock.calls[1][0]).not.toHaveProperty('retry_of')
  })

  it('keeps absent lookup unknown and overrides an application-wide automatic retry setting', async () => {
    execute.mockRejectedValue(failure())
    lookup.mockResolvedValue(null)
    const client = new QueryClient({ defaultOptions: { mutations: { retry: 3, retryDelay: 1 } } })
    renderWithProviders(
      <QueryClientProvider client={client}>
        <IntentHintBar status={status} />
      </QueryClientProvider>,
    )
    await submit()
    await settled()
    await observe()
    await waitFor(() => expect(screen.getByText(/No unique matching observation/)).toBeInTheDocument())
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20))
    })
    expect(execute).toHaveBeenCalledOnce()
    expect(lookup).toHaveBeenCalledOnce()
    expect(screen.getByTestId('intent-provenance-unknown')).toBeInTheDocument()
    expect(screen.getByText(/may duplicate the earlier one/)).toBeInTheDocument()
    expect(document.body.textContent).not.toContain(PRIVATE_CANARY)
  })

  it('keeps the observed fallback after provider health changes and loses that claim when provenance is removed', async () => {
    execute.mockResolvedValueOnce(makeResponse(makeFallback())).mockResolvedValueOnce(makeResponse(null))
    const { rerender } = renderWithProviders(<IntentHintBar status={status} />)
    await submit()
    await settled()
    expect(screen.getByTestId('intent-observed-plan')).toHaveTextContent('Rule-based plan')
    rerender(
      <IntentHintBar
        status={{ enabled: true, llm_source: 'healthy-local-model', llm_fallback_reason: null } as never}
      />,
    )
    expect(screen.getByTestId('intent-observed-plan')).toHaveTextContent('Rule-based plan')
    await submit()
    await settled()
    expect(screen.getByTestId('intent-provenance-unknown')).toBeInTheDocument()
    expect(screen.queryByTestId('intent-observed-plan')).not.toBeInTheDocument()
  })

  it('displays safe error observations and never raw provider, executor or correlation errors', async () => {
    execute
      .mockRejectedValueOnce(failure(makeProvenance({ planning_outcome: 'denied', action_outcome: 'not_started' })))
      .mockResolvedValueOnce({
        ...makeResponse(null),
        result: { ...makeResponse().result, success: false, error: PRIVATE_CANARY },
      })
    renderWithProviders(<IntentHintBar status={status} />)
    await submit()
    await settled()
    expect(screen.getByTestId('intent-observed-plan')).toHaveTextContent('Planning denied')
    expect(document.body.textContent).not.toContain(PRIVATE_CANARY)
    await submit()
    await settled()
    expect(screen.getByText('Command failed')).toBeInTheDocument()
    expect(document.body.textContent).not.toContain(PRIVATE_CANARY)
    vi.mocked(crypto.randomUUID).mockImplementation(() => {
      throw new Error(PRIVATE_CANARY)
    })
    await submit()
    expect(screen.getByText('A secure submission ID could not be created. Nothing was sent.')).toBeInTheDocument()
    expect(execute).toHaveBeenCalledTimes(2)
    expect(document.body.textContent).not.toContain(PRIVATE_CANARY)
  })
})
