import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useUpdateGoals } from '../useCoaching'

// Mock the network layer so the mutation resolves without a real request.
vi.mock('../../api/coaching', () => ({
  updateRegimeGoals: vi.fn().mockResolvedValue(undefined),
}))

// Silence toast side-effects (not under test here).
vi.mock('../useToast', () => ({
  addToast: vi.fn(),
}))

function makeWrapper(queryClient: QueryClient) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  }
}

describe('useUpdateGoals', () => {
  let queryClient: QueryClient

  beforeEach(() => {
    queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    })
  })

  // #8083: adding/removing a goal must refresh BOTH the goal-progress view AND
  // the settings query, because regime_goals is also carried (display-only) in
  // the settings payload — otherwise the Settings form keeps a stale goal list.
  it('invalidates goal-progress AND settings queries on success', async () => {
    const invalidateSpy = vi.spyOn(queryClient, 'invalidateQueries')
    const { result } = renderHook(() => useUpdateGoals(), {
      wrapper: makeWrapper(queryClient),
    })

    result.current.mutate({ deep_work: 180 })

    await waitFor(() => expect(result.current.isSuccess).toBe(true))

    const invalidatedKeys = invalidateSpy.mock.calls.map((call) => call[0]?.queryKey?.[0])
    expect(invalidatedKeys).toContain('goal-progress')
    expect(invalidatedKeys).toContain('settings')
  })
})
