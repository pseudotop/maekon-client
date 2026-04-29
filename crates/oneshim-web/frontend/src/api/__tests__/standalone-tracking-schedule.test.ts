import { afterEach, describe, expect, it, vi } from 'vitest'

const STANDALONE_STORAGE_KEY = 'oneshim-web-standalone-mode'

describe('standalone tracking schedule API', () => {
  const originalUrl = window.location.href

  afterEach(() => {
    window.localStorage.clear()
    window.history.replaceState({}, '', originalUrl)
    vi.resetModules()
  })

  it('returns a typed default tracking schedule instead of the generic ok fallback', async () => {
    window.localStorage.setItem(STANDALONE_STORAGE_KEY, '1')
    const { handleStandaloneRequest } = await import('../standalone')

    const response = await handleStandaloneRequest('/api/tracking-schedule', undefined, true)
    const body = await response?.json()

    expect(response?.ok).toBe(true)
    expect(body).toEqual({
      enabled: false,
      windows: [],
      timezone: 'Local',
    })
  })

  it('stores tracking schedule changes for standalone UI smoke checks', async () => {
    window.localStorage.setItem(STANDALONE_STORAGE_KEY, '1')
    const { handleStandaloneRequest } = await import('../standalone')

    const schedule = {
      enabled: true,
      windows: [{ start: '09:00', end: '17:00', days_of_week: ['Mon', 'Tue'], label: 'Work' }],
      timezone: 'Asia/Seoul',
    }

    await handleStandaloneRequest(
      '/api/tracking-schedule',
      {
        method: 'PUT',
        body: JSON.stringify(schedule),
      },
      true,
    )

    const response = await handleStandaloneRequest('/api/tracking-schedule', undefined, true)
    const body = await response?.json()

    expect(body).toEqual(schedule)
  })

  it('returns a typed default tracking schedule status', async () => {
    window.localStorage.setItem(STANDALONE_STORAGE_KEY, '1')
    const { handleStandaloneRequest } = await import('../standalone')

    const response = await handleStandaloneRequest('/api/tracking-schedule/status', undefined, true)
    const body = await response?.json()

    expect(response?.ok).toBe(true)
    expect(body).toEqual({
      active_now: false,
      ends_at: null,
      next_starts_at: null,
      label: '',
    })
  })
})
