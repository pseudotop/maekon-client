import { Moon, Pause, Play, SkipBack, SkipForward } from 'lucide-react'
import { useCallback, useMemo, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import type { AppSegment, TimelineItem } from '../api/client'
import { iconSize, motion, palette, typography } from '../styles/tokens'
import { formatTime } from '../utils/formatters'
import { Select } from './ui'

interface TimelineScrubberProps {
  startTime: Date
  endTime: Date
  currentTime: Date
  isPlaying: boolean
  playbackSpeed: number
  segments: AppSegment[]
  items: TimelineItem[]
  onTimeChange: (time: Date) => void
  onPlayPause: () => void
  onSpeedChange: (speed: number) => void
  onSkipToStart: () => void
  onSkipToEnd: () => void
}

const PLAYBACK_SPEEDS = [0.5, 1, 2, 5, 10]

export default function TimelineScrubber({
  startTime,
  endTime,
  currentTime,
  isPlaying,
  playbackSpeed,
  segments,
  items,
  onTimeChange,
  onPlayPause,
  onSpeedChange,
  onSkipToStart,
  onSkipToEnd,
}: TimelineScrubberProps) {
  const { t, i18n } = useTranslation()
  const locale = i18n.resolvedLanguage ?? i18n.language
  const trackRef = useRef<HTMLDivElement>(null)

  const totalDuration = endTime.getTime() - startTime.getTime()

  const currentPosition = totalDuration > 0 ? (currentTime.getTime() - startTime.getTime()) / totalDuration : 0

  const idlePeriods = useMemo(() => {
    return items
      .filter((item): item is Extract<TimelineItem, { type: 'IdlePeriod' }> => item.type === 'IdlePeriod')
      .map((item) => ({
        start: new Date(item.start).getTime(),
        end: new Date(item.end).getTime(),
      }))
  }, [items])

  const handleTrackClick = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!trackRef.current || totalDuration <= 0) return

      const rect = trackRef.current.getBoundingClientRect()
      const x = e.clientX - rect.left
      const ratio = Math.max(0, Math.min(1, x / rect.width))
      const newTime = new Date(startTime.getTime() + ratio * totalDuration)
      onTimeChange(newTime)
    },
    [startTime, totalDuration, onTimeChange],
  )

  const handleMouseDown = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!trackRef.current || totalDuration <= 0) return

      // Cache rect once on mousedown to avoid layout thrashing during drag
      const cachedRect = trackRef.current.getBoundingClientRect()

      const handleMouseMove = (moveEvent: MouseEvent) => {
        const x = moveEvent.clientX - cachedRect.left
        const ratio = Math.max(0, Math.min(1, x / cachedRect.width))
        const newTime = new Date(startTime.getTime() + ratio * totalDuration)
        onTimeChange(newTime)
      }

      const handleMouseUp = () => {
        document.removeEventListener('mousemove', handleMouseMove)
        document.removeEventListener('mouseup', handleMouseUp)
      }

      document.addEventListener('mousemove', handleMouseMove)
      document.addEventListener('mouseup', handleMouseUp)

      handleTrackClick(e)
    },
    [startTime, totalDuration, onTimeChange, handleTrackClick],
  )

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (totalDuration <= 0) return

      const STEP_MS = 60_000 // 1 minute
      const SHIFT_STEP_MS = 300_000 // 5 minutes
      const step = e.shiftKey ? SHIFT_STEP_MS : STEP_MS
      let newTime: Date | null = null

      switch (e.key) {
        case 'ArrowLeft':
          newTime = new Date(Math.max(startTime.getTime(), currentTime.getTime() - step))
          break
        case 'ArrowRight':
          newTime = new Date(Math.min(endTime.getTime(), currentTime.getTime() + step))
          break
        case 'Home':
          newTime = new Date(startTime.getTime())
          break
        case 'End':
          newTime = new Date(endTime.getTime())
          break
        default:
          return
      }

      e.preventDefault()
      onTimeChange(newTime)
    },
    [startTime, endTime, currentTime, totalDuration, onTimeChange],
  )

  return (
    <div className="rounded-lg border border-muted bg-surface-overlay p-4 shadow">
      {/* UI note */}
      <div className="mb-3 flex items-center justify-between">
        <div className="flex items-center space-x-2">
          {/* UI note */}
          <button
            type="button"
            data-testid="replay-start"
            onClick={onSkipToStart}
            className={`rounded-lg p-2 text-content-secondary ${motion.colors} hover:bg-hover`}
            title={t('replay.skipToStart', '처음으로')}
          >
            <SkipBack className={iconSize.md} />
          </button>

          {/* UI note */}
          <button
            type="button"
            data-testid="replay-play"
            onClick={onPlayPause}
            className={`rounded-lg bg-brand p-2 text-content-inverse ${motion.colors} hover:bg-brand-hover`}
            title={isPlaying ? t('replay.pause', '일시정지') : t('replay.play', '재생')}
          >
            {isPlaying ? <Pause className={iconSize.md} /> : <Play className={iconSize.md} />}
          </button>

          {/* UI note */}
          <button
            type="button"
            data-testid="replay-end"
            onClick={onSkipToEnd}
            className={`rounded-lg p-2 text-content-secondary ${motion.colors} hover:bg-hover`}
            title={t('replay.skipToEnd', '끝으로')}
          >
            <SkipForward className={iconSize.md} />
          </button>

          {/* UI note */}
          <span className={`ml-3 ${typography.family.mono} text-content-strong text-sm`}>
            {formatTime(currentTime.toISOString(), locale)}
          </span>
        </div>

        {/* UI note */}
        <div className="flex items-center space-x-2">
          <span className="text-content-secondary text-xs">{t('replay.speed', '재생 속도')}:</span>
          <Select
            data-testid="replay-speed"
            value={playbackSpeed}
            selectSize="sm"
            onChange={(e) => onSpeedChange(Number(e.target.value))}
            className="w-auto"
          >
            {PLAYBACK_SPEEDS.map((speed) => (
              <option key={speed} value={speed} data-testid={`replay-speed-${speed}x`}>
                {speed}x
              </option>
            ))}
          </Select>
        </div>
      </div>

      {/* UI note */}
      <div
        ref={trackRef}
        role="slider"
        tabIndex={0}
        aria-label={t('replay.scrubber', 'Timeline scrubber')}
        aria-valuemin={0}
        aria-valuemax={totalDuration}
        aria-valuenow={currentTime.getTime() - startTime.getTime()}
        aria-valuetext={formatTime(currentTime.toISOString(), locale)}
        className="relative h-10 cursor-pointer overflow-hidden rounded-lg bg-hover"
        onClick={handleTrackClick}
        onMouseDown={handleMouseDown}
        onKeyDown={handleKeyDown}
      >
        {/* UI note */}
        {segments.map((segment) => {
          const segmentStart = new Date(segment.start).getTime()
          const segmentEnd = new Date(segment.end).getTime()
          const left = totalDuration > 0 ? ((segmentStart - startTime.getTime()) / totalDuration) * 100 : 0
          const width = totalDuration > 0 ? ((segmentEnd - segmentStart) / totalDuration) * 100 : 0

          return (
            <div
              key={`segment-${segment.app_name}-${segment.start}`}
              className={`absolute top-0 h-full ${motion.opacity} hover:opacity-80`}
              style={{
                left: `${left}%`,
                width: `${Math.max(width, 0.5)}%`,
                backgroundColor: segment.color,
              }}
              title={segment.app_name}
            />
          )
        })}

        {/* UI note */}
        {idlePeriods.map((idle) => {
          const left = totalDuration > 0 ? ((idle.start - startTime.getTime()) / totalDuration) * 100 : 0
          const width = totalDuration > 0 ? ((idle.end - idle.start) / totalDuration) * 100 : 0

          return (
            <div
              key={`idle-${idle.start}-${idle.end}`}
              className="absolute top-0 flex h-full items-center justify-center"
              style={{
                left: `${left}%`,
                width: `${Math.max(width, 0.5)}%`,
                background:
                  'repeating-linear-gradient(45deg, rgb(var(--content-muted) / 0.3), rgb(var(--content-muted) / 0.3) 2px, transparent 2px, transparent 4px)',
              }}
              title={t('replay.idle', 'idle')}
            >
              {width > 3 && <Moon className={`${iconSize.xs} text-content-secondary opacity-70`} />}
            </div>
          )
        })}

        {/* UI note */}
        <div
          className="absolute top-0 z-10 h-full w-0.5 bg-semantic-error shadow-lg"
          style={{ left: `${currentPosition * 100}%` }}
        >
          {/* UI note */}
          <div className={`absolute -top-1 -left-1.5 ${iconSize.base} rounded-full bg-semantic-error shadow-lg`} />
        </div>

        {/* UI note */}
        <div className="absolute bottom-0 left-1 text-content-secondary text-xs opacity-70">
          {formatTime(startTime.toISOString(), locale)}
        </div>
        <div className="absolute right-1 bottom-0 text-content-secondary text-xs opacity-70">
          {formatTime(endTime.toISOString(), locale)}
        </div>
      </div>

      {/* UI note */}
      {segments.length > 0 && (
        <div className="mt-3 flex flex-wrap gap-2">
          {/* UI note */}
          {Array.from(new Set(segments.map((s) => s.app_name)))
            .slice(0, 8)
            .map((appName) => {
              const segment = segments.find((s) => s.app_name === appName)
              return (
                <div key={appName} className="flex items-center space-x-1">
                  <div
                    className={`${iconSize.xs} rounded-sm`}
                    style={{ backgroundColor: segment?.color || palette.gray500 }}
                  />
                  <span className="max-w-[80px] truncate text-content-secondary text-xs">{appName}</span>
                </div>
              )
            })}
          {/* UI note */}
          {idlePeriods.length > 0 && (
            <div className="flex items-center space-x-1">
              <div
                className={`${iconSize.xs} rounded-sm`}
                style={{
                  background:
                    'repeating-linear-gradient(45deg, rgb(var(--content-muted) / 0.5), rgb(var(--content-muted) / 0.5) 1px, transparent 1px, transparent 2px)',
                }}
              />
              <span className="text-content-secondary text-xs">{t('replay.idle', 'idle')}</span>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
