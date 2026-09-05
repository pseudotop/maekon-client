import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { FeatureCapabilitySnapshot } from '../api/contracts'
import { invalidateAiReadinessSnapshotCache, useAiReadinessSnapshot } from './useAiReadinessSnapshot'

const mocks = vi.hoisted(() => ({
  fetchFeatureCapabilities: vi.fn(),
}))

vi.mock('../api/client', () => ({ fetchFeatureCapabilities: mocks.fetchFeatureCapabilities }))
vi.mock('../api/standalone', () => ({ isStandaloneModeEnabled: () => false }))

const firstSnapshot: FeatureCapabilitySnapshot = {
  features: [],
  ai_readiness: { contract_version: 1, capabilities: [] },
}

describe('useAiReadinessSnapshot', () => {
  beforeEach(() => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true })
    mocks.fetchFeatureCapabilities.mockReset()
    invalidateAiReadinessSnapshotCache()
  })

  it('deduplicates the desktop read and refreshes every mounted consumer after invalidation', async () => {
    mocks.fetchFeatureCapabilities.mockResolvedValueOnce(firstSnapshot)

    const first = renderHook(() => useAiReadinessSnapshot())
    const second = renderHook(() => useAiReadinessSnapshot())

    await waitFor(() => expect(first.result.current).toBe(firstSnapshot))
    await waitFor(() => expect(second.result.current).toBe(firstSnapshot))
    expect(mocks.fetchFeatureCapabilities).toHaveBeenCalledOnce()

    const refreshedSnapshot = { ...firstSnapshot, audio_compiled: true }
    mocks.fetchFeatureCapabilities.mockResolvedValueOnce(refreshedSnapshot)
    act(() => invalidateAiReadinessSnapshotCache())

    await waitFor(() => expect(first.result.current).toBe(refreshedSnapshot))
    await waitFor(() => expect(second.result.current).toBe(refreshedSnapshot))
    expect(mocks.fetchFeatureCapabilities).toHaveBeenCalledTimes(2)
  })

  it('does not let an invalidated in-flight response overwrite the refreshed snapshot', async () => {
    let resolveStale: ((snapshot: FeatureCapabilitySnapshot) => void) | undefined
    mocks.fetchFeatureCapabilities.mockImplementationOnce(
      () =>
        new Promise<FeatureCapabilitySnapshot>((resolve) => {
          resolveStale = resolve
        }),
    )
    const hook = renderHook(() => useAiReadinessSnapshot())
    await waitFor(() => expect(mocks.fetchFeatureCapabilities).toHaveBeenCalledOnce())

    const refreshedSnapshot = { ...firstSnapshot, audio_compiled: true }
    mocks.fetchFeatureCapabilities.mockResolvedValueOnce(refreshedSnapshot)
    act(() => invalidateAiReadinessSnapshotCache())
    await waitFor(() => expect(hook.result.current).toBe(refreshedSnapshot))

    act(() => resolveStale?.(firstSnapshot))
    await act(async () => Promise.resolve())
    expect(hook.result.current).toBe(refreshedSnapshot)
    expect(mocks.fetchFeatureCapabilities).toHaveBeenCalledTimes(2)
  })

  it('fails closed and refreshes a mounted consumer when the cache expires', async () => {
    vi.useFakeTimers()
    try {
      mocks.fetchFeatureCapabilities.mockResolvedValueOnce(firstSnapshot)
      const refreshedSnapshot = { ...firstSnapshot, audio_compiled: true }
      let resolveRefresh: ((snapshot: FeatureCapabilitySnapshot) => void) | undefined
      mocks.fetchFeatureCapabilities.mockImplementationOnce(
        () =>
          new Promise<FeatureCapabilitySnapshot>((resolve) => {
            resolveRefresh = resolve
          }),
      )

      const hook = renderHook(() => useAiReadinessSnapshot())
      await act(async () => Promise.resolve())
      expect(hook.result.current).toBe(firstSnapshot)

      await act(async () => vi.advanceTimersByTimeAsync(300_000))
      expect(hook.result.current).toBeUndefined()
      expect(mocks.fetchFeatureCapabilities).toHaveBeenCalledTimes(2)

      await act(async () => resolveRefresh?.(refreshedSnapshot))
      expect(hook.result.current).toBe(refreshedSnapshot)
    } finally {
      vi.useRealTimers()
    }
  })

  it('retries after a transient desktop capability read failure', async () => {
    vi.useFakeTimers()
    try {
      mocks.fetchFeatureCapabilities.mockRejectedValueOnce(new Error('temporary IPC failure'))
      mocks.fetchFeatureCapabilities.mockResolvedValueOnce(firstSnapshot)

      const hook = renderHook(() => useAiReadinessSnapshot())
      await act(async () => Promise.resolve())
      expect(hook.result.current).toBeUndefined()
      expect(mocks.fetchFeatureCapabilities).toHaveBeenCalledOnce()

      await act(async () => vi.advanceTimersByTimeAsync(30_000))
      expect(hook.result.current).toBe(firstSnapshot)
      expect(mocks.fetchFeatureCapabilities).toHaveBeenCalledTimes(2)
    } finally {
      vi.useRealTimers()
    }
  })
})
