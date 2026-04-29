import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('../../api/standalone', () => ({
  isStandaloneModeEnabled: vi.fn(() => true),
}))

vi.mock('../../utils/api-base', () => ({
  resolveApiUrl: vi.fn(async (url: string) => url),
}))

import { isStandaloneModeEnabled } from '../../api/standalone'
import { resolveApiUrl } from '../../utils/api-base'
import { useSSE } from '../useSSE'

const mockIsStandaloneModeEnabled = vi.mocked(isStandaloneModeEnabled)
const mockResolveApiUrl = vi.mocked(resolveApiUrl)

describe('useSSE', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockIsStandaloneModeEnabled.mockReturnValue(true)
    mockResolveApiUrl.mockImplementation(async (url: string) => url)
  })

  it('returns expected shape', () => {
    const { result } = renderHook(() => useSSE())
    expect(result.current).toHaveProperty('status')
    expect(result.current).toHaveProperty('latestMetrics')
    expect(result.current).toHaveProperty('latestFrame')
    expect(result.current).toHaveProperty('idleState')
    expect(result.current).toHaveProperty('metricsHistory')
    expect(result.current).toHaveProperty('connect')
    expect(result.current).toHaveProperty('disconnect')
  })

  it('initial status is disconnected (standalone mode)', () => {
    const { result } = renderHook(() => useSSE())
    expect(result.current.status).toBe('disconnected')
  })

  it('initial metrics are null', () => {
    const { result } = renderHook(() => useSSE())
    expect(result.current.latestMetrics).toBeNull()
  })

  it('initial metricsHistory is empty', () => {
    const { result } = renderHook(() => useSSE())
    expect(result.current.metricsHistory).toEqual([])
  })

  it('connect and disconnect are functions', () => {
    const { result } = renderHook(() => useSSE())
    expect(typeof result.current.connect).toBe('function')
    expect(typeof result.current.disconnect).toBe('function')
  })

  it('disconnect prevents late EventSource creation while URL resolution is pending', async () => {
    mockIsStandaloneModeEnabled.mockReturnValue(false)

    let resolveUrl: ((value: string) => void) | undefined
    mockResolveApiUrl.mockReturnValueOnce(
      new Promise<string>((resolve) => {
        resolveUrl = resolve
      }),
    )

    const createdUrls: string[] = []
    const BaseEventSource = globalThis.EventSource
    class CountingEventSource extends BaseEventSource {
      constructor(url: string | URL) {
        super(url)
        createdUrls.push(typeof url === 'string' ? url : url.toString())
      }
    }
    Object.defineProperty(globalThis, 'EventSource', {
      value: CountingEventSource,
      writable: true,
    })

    try {
      const { result } = renderHook(() => useSSE())

      await act(async () => {
        await Promise.resolve()
      })
      act(() => {
        result.current.disconnect()
      })
      await act(async () => {
        resolveUrl?.('/api/stream')
        await Promise.resolve()
      })

      expect(createdUrls).toEqual([])
      expect(result.current.status).toBe('disconnected')
    } finally {
      Object.defineProperty(globalThis, 'EventSource', {
        value: BaseEventSource,
        writable: true,
      })
    }
  })
})
