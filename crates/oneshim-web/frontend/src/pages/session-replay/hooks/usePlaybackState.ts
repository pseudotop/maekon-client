import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { TimelineItem } from '../../../api/client'
import type { FrameItem, PlaybackState } from '../types'

interface TimelineData {
  session: { start: string; end: string }
  items: TimelineItem[]
}

const PLAYBACK_TICK_MS = 100

function getMonotonicNowMs(): number {
  return typeof performance !== 'undefined' && typeof performance.now === 'function' ? performance.now() : Date.now()
}

/**
 * Encapsulates all playback-specific state: play/pause, speed, currentTime,
 * frame lookup, and keyboard-style navigation (skip to start/end).
 */
export function usePlaybackState(timeline: TimelineData | undefined): PlaybackState {
  const [isPlaying, setIsPlaying] = useState(false)
  const [playbackSpeed, setPlaybackSpeed] = useState(1)
  const [currentTime, setCurrentTime] = useState<Date>(new Date())

  const playIntervalRef = useRef<number | null>(null)
  const currentTimeRef = useRef(currentTime.getTime())
  const playbackAnchorRef = useRef<{ wallClockMs: number; timelineMs: number } | null>(null)

  const sessionStart = timeline?.session.start
  const sessionEnd = timeline?.session.end
  const hasTimeline = Boolean(timeline)

  const syncCurrentTime = useCallback((time: Date) => {
    currentTimeRef.current = time.getTime()
    setCurrentTime(time)
  }, [])

  const resetPlaybackAnchor = useCallback((timelineMs = currentTimeRef.current) => {
    playbackAnchorRef.current = {
      wallClockMs: getMonotonicNowMs(),
      timelineMs,
    }
  }, [])

  useEffect(() => {
    currentTimeRef.current = currentTime.getTime()
  }, [currentTime])

  // Sync currentTime to session start when a different session loads.
  useEffect(() => {
    if (sessionStart) {
      syncCurrentTime(new Date(sessionStart))
      playbackAnchorRef.current = null
    }
  }, [sessionStart, syncCurrentTime])

  const startTime = useMemo(() => {
    return sessionStart ? new Date(sessionStart) : new Date()
  }, [sessionStart])

  const endTime = useMemo(() => {
    return sessionEnd ? new Date(sessionEnd) : new Date()
  }, [sessionEnd])

  const endTimeMs = endTime.getTime()

  // Find the closest frame at or before currentTime
  const currentFrame = useMemo((): FrameItem | null => {
    if (!timeline?.items) return null

    const frames = timeline.items.filter((item): item is FrameItem => item.type === 'Frame')

    if (frames.length === 0) return null

    const currentMs = currentTime.getTime()

    let closest: FrameItem | null = null
    let closestDiff = Infinity

    for (const frame of frames) {
      const frameMs = new Date(frame.timestamp).getTime()
      if (frameMs <= currentMs) {
        const diff = currentMs - frameMs
        if (diff < closestDiff) {
          closestDiff = diff
          closest = frame
        }
      }
    }

    return closest
  }, [timeline, currentTime])

  // Playback interval timer
  useEffect(() => {
    if (!isPlaying || !hasTimeline) {
      if (playIntervalRef.current !== null) {
        clearInterval(playIntervalRef.current)
        playIntervalRef.current = null
      }
      playbackAnchorRef.current = null
      return undefined
    }

    resetPlaybackAnchor()

    const tick = () => {
      const anchor = playbackAnchorRef.current
      if (!anchor) return

      const elapsedMs = Math.max(0, getMonotonicNowMs() - anchor.wallClockMs)
      const nextMs = anchor.timelineMs + elapsedMs * playbackSpeed

      setCurrentTime(() => {
        if (nextMs >= endTimeMs) {
          setIsPlaying(false)
          playbackAnchorRef.current = null
          currentTimeRef.current = endTimeMs
          return new Date(endTimeMs)
        }

        currentTimeRef.current = nextMs
        return new Date(nextMs)
      })
    }

    const interval = window.setInterval(() => {
      tick()
    }, PLAYBACK_TICK_MS)

    playIntervalRef.current = interval

    return () => {
      clearInterval(interval)
      if (playIntervalRef.current === interval) {
        playIntervalRef.current = null
      }
    }
  }, [isPlaying, playbackSpeed, endTimeMs, hasTimeline, resetPlaybackAnchor])

  const handlePlayPause = useCallback(() => {
    setIsPlaying((prev) => {
      if (prev) {
        playbackAnchorRef.current = null
        return false
      }

      resetPlaybackAnchor()
      return true
    })
  }, [resetPlaybackAnchor])

  const handleSpeedChange = useCallback(
    (speed: number) => {
      if (isPlaying) {
        resetPlaybackAnchor()
      }
      setPlaybackSpeed(speed)
    },
    [isPlaying, resetPlaybackAnchor],
  )

  const handleTimeChange = useCallback(
    (time: Date) => {
      syncCurrentTime(time)
      if (isPlaying) {
        resetPlaybackAnchor(time.getTime())
      }
    },
    [isPlaying, resetPlaybackAnchor, syncCurrentTime],
  )

  const handleSkipToStart = useCallback(() => {
    syncCurrentTime(startTime)
    setIsPlaying(false)
    playbackAnchorRef.current = null
  }, [startTime, syncCurrentTime])

  const handleSkipToEnd = useCallback(() => {
    syncCurrentTime(endTime)
    setIsPlaying(false)
    playbackAnchorRef.current = null
  }, [endTime, syncCurrentTime])

  return {
    isPlaying,
    playbackSpeed,
    currentTime,
    startTime,
    endTime,
    currentFrame,
    handlePlayPause,
    handleSpeedChange,
    handleTimeChange,
    handleSkipToStart,
    handleSkipToEnd,
  }
}
