import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { TimelineItem } from '../../../api/client'
import { usePlaybackState } from './usePlaybackState'

const session = {
  start: '2026-04-27T09:00:00.000Z',
  end: '2026-04-27T09:10:00.000Z',
}

let monotonicNowMs = 0

function createTimeline(items: TimelineItem[] = []) {
  return { session, items }
}

function advancePlaybackTime(ms: number) {
  let remainingMs = ms

  while (remainingMs > 0) {
    const deltaMs = Math.min(100, remainingMs)
    monotonicNowMs += deltaMs
    vi.advanceTimersByTime(deltaMs)
    remainingMs -= deltaMs
  }
}

describe('usePlaybackState', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date(session.start))
    monotonicNowMs = 0
    vi.spyOn(performance, 'now').mockImplementation(() => monotonicNowMs)
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('keeps 1x playback at one timeline second per real second across timeline refetches', () => {
    const { result, rerender } = renderHook(({ timeline }) => usePlaybackState(timeline), {
      initialProps: { timeline: createTimeline() },
    })

    act(() => {
      result.current.handleSkipToStart()
    })
    act(() => {
      result.current.handlePlayPause()
    })
    act(() => {
      advancePlaybackTime(5_000)
    })

    expect(result.current.currentTime.toISOString()).toBe('2026-04-27T09:00:05.000Z')

    rerender({ timeline: createTimeline() })

    act(() => {
      advancePlaybackTime(1_000)
    })

    expect(result.current.currentTime.toISOString()).toBe('2026-04-27T09:00:06.000Z')
  })

  it('derives 1x playback from wall-clock time when timer callbacks fire too often', () => {
    const realSetInterval = window.setInterval.bind(window)
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      _timeout?: number,
      ...args: unknown[]
    ) => realSetInterval(handler, 100, ...args)) as typeof window.setInterval)

    const { result } = renderHook(({ timeline }) => usePlaybackState(timeline), {
      initialProps: { timeline: createTimeline() },
    })

    act(() => {
      result.current.handleSkipToStart()
    })
    act(() => {
      result.current.handlePlayPause()
    })
    act(() => {
      advancePlaybackTime(1_000)
    })

    expect(result.current.currentTime.toISOString()).toBe('2026-04-27T09:00:01.000Z')
  })

  it('keeps playback on monotonic time when the system clock jumps forward', () => {
    const { result } = renderHook(({ timeline }) => usePlaybackState(timeline), {
      initialProps: { timeline: createTimeline() },
    })

    act(() => {
      result.current.handleSkipToStart()
    })
    act(() => {
      result.current.handlePlayPause()
    })

    vi.setSystemTime(new Date(session.start).getTime() + 60_000)

    act(() => {
      advancePlaybackTime(1_000)
    })

    expect(result.current.currentTime.toISOString()).toBe('2026-04-27T09:00:01.000Z')
  })
})
