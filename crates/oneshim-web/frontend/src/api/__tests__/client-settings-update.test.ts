import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('../../utils/api-base', () => ({
  resolveApiUrl: vi.fn(async (url: string) => url),
}))

vi.mock('../standalone', () => ({
  handleStandaloneRequest: vi.fn(async () => null),
  isStandaloneModeEnabled: vi.fn(() => false),
}))

import { fetchMetrics, fetchSettings, fetchUpdateStatus, postUpdateAction } from '../client'

describe('api client settings/update transport', () => {
  const fetchMock = vi.fn<typeof fetch>()

  beforeEach(() => {
    fetchMock.mockReset()
    vi.stubGlobal('fetch', fetchMock)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('fetchSettings requests /api/settings', async () => {
    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify({ monitoring: { interval_seconds: 5 } }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )

    const result = await fetchSettings()

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/settings',
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    )
    expect(result).toMatchObject({ monitoring: { interval_seconds: 5 } })
  })

  it('fetchMetrics requests /api/metrics with query params', async () => {
    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify([{ timestamp: '2026-03-31T00:00:00Z', cpu_usage: 12.5 }]), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )

    const result = await fetchMetrics('2026-03-30T00:00:00Z', '2026-03-31T00:00:00Z', 50)

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/metrics?from=2026-03-30T00%3A00%3A00Z&to=2026-03-31T00%3A00%3A00Z&limit=50',
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    )
    expect(result[0]).toMatchObject({ cpu_usage: 12.5 })
  })

  it('fetchUpdateStatus requests /api/update/status', async () => {
    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify({ phase: 'idle' }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )

    const result = await fetchUpdateStatus()

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/update/status',
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    )
    expect(result.phase).toBe('idle')
  })

  it('postUpdateAction posts JSON to /api/update/action', async () => {
    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify({ accepted: true, status: { phase: 'idle' } }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )

    const result = await postUpdateAction('Approve')

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/update/action',
      expect.objectContaining({
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'Approve' }),
        signal: expect.any(AbortSignal),
      }),
    )
    expect(result.accepted).toBe(true)
  })
})
