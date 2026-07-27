import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('../../api/standalone', () => ({
  isStandaloneModeEnabled: vi.fn(() => false),
}))

vi.mock('../../utils/api-base', () => ({
  resolveApiUrl: vi.fn(async (url: string) => url),
  resolveLocalAuthToken: vi.fn(async () => ''),
  setLocalAuthCookie: vi.fn(),
  withLocalAuthQuery: vi.fn((url: string) => url),
}))

import { useUpdateStream } from '../useUpdateStream'

type Listener = (event: MessageEvent) => void

class ControlledEventSource {
  static readonly CONNECTING = 0
  static readonly OPEN = 1
  static readonly CLOSED = 2
  static instances: ControlledEventSource[] = []

  readonly CONNECTING = ControlledEventSource.CONNECTING
  readonly OPEN = ControlledEventSource.OPEN
  readonly CLOSED = ControlledEventSource.CLOSED
  readonly url: string
  readyState = ControlledEventSource.CONNECTING
  onopen: ((event: Event) => void) | null = null
  onerror: ((event: Event) => void) | null = null
  private listeners = new Map<string, Set<Listener>>()

  constructor(url: string | URL) {
    this.url = typeof url === 'string' ? url : url.toString()
    ControlledEventSource.instances.push(this)
  }

  addEventListener(type: string, listener: EventListener) {
    const listeners = this.listeners.get(type) ?? new Set<Listener>()
    listeners.add(listener as Listener)
    this.listeners.set(type, listeners)
  }

  close() {
    this.readyState = ControlledEventSource.CLOSED
  }

  open() {
    this.readyState = ControlledEventSource.OPEN
    this.onopen?.(new Event('open'))
  }

  fail() {
    this.onerror?.(new Event('error'))
  }

  emit(type: string, data: unknown) {
    const event = new MessageEvent(type, { data: JSON.stringify(data) })
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event)
    }
  }
}

async function flushConnection() {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

describe('useUpdateStream recovery', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    ControlledEventSource.instances = []
    Object.defineProperty(globalThis, 'EventSource', {
      value: ControlledEventSource,
      writable: true,
    })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('recovers after two drops with one active stream and ignores replayed revisions', async () => {
    const { result, unmount } = renderHook(() => useUpdateStream())
    await flushConnection()

    expect(ControlledEventSource.instances).toHaveLength(1)
    act(() => ControlledEventSource.instances[0].fail())
    expect(result.current.status).toBe('error')
    expect(result.current.retryCount).toBe(1)

    await act(async () => vi.advanceTimersByTimeAsync(2000))
    await flushConnection()
    expect(ControlledEventSource.instances).toHaveLength(2)
    act(() => ControlledEventSource.instances[1].fail())
    expect(result.current.retryCount).toBe(2)

    await act(async () => vi.advanceTimersByTimeAsync(2000))
    await flushConnection()
    expect(ControlledEventSource.instances).toHaveLength(3)
    act(() => ControlledEventSource.instances[2].open())

    expect(result.current.status).toBe('connected')
    expect(result.current.retryCount).toBe(0)
    expect(result.current.recoveredAt).not.toBeNull()
    expect(ControlledEventSource.instances.filter((source) => source.readyState === source.OPEN)).toHaveLength(1)

    const update = {
      enabled: true,
      auto_install: false,
      phase: 'Updated',
      message: 'Synthetic recovery status',
      pending: null,
      download_progress: null,
      rollback: null,
      revision: 7,
      updated_at: '2026-07-19T03:00:00Z',
    }
    act(() => ControlledEventSource.instances[2].emit('update_status', update))
    const firstObject = result.current.latest
    const firstEventAt = result.current.lastEventAt

    await act(async () => vi.advanceTimersByTimeAsync(1000))
    act(() => ControlledEventSource.instances[2].emit('update_status', update))

    expect(result.current.latest).toBe(firstObject)
    expect(result.current.lastEventAt).toBe(firstEventAt)
    expect(ControlledEventSource.instances[0].readyState).toBe(ControlledEventSource.CLOSED)
    expect(ControlledEventSource.instances[1].readyState).toBe(ControlledEventSource.CLOSED)

    unmount()
  })
})
