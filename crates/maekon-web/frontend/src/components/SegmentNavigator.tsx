import { useCallback, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import type { AppSegment } from '../api/contracts'
import { motion, palette, typography } from '../styles/tokens'

/**
 * Time-axis navigation over app segments (#9812).
 *
 * Why this exists rather than more scrolling: at the default 5s capture
 * throttle and 30-day retention a install holds on the order of 170,000
 * frames, so 50-per-page paging is ~3,400 pages and infinite scroll is no
 * better — neither is a distance a person crosses. Segments are the unit that
 * makes the question answerable: ~10 minutes each, so ~1,400 over the same
 * retention, which renders without virtualization.
 *
 * It also answers the question the frame list structurally cannot. "Which task
 * was on screen" is not a property of a frame; it lives on the segment
 * (`app_name` here, and `dominant_category` / `llm_summary` on the record
 * behind it).
 *
 * Deliberately NOT the existing `TimelineScrubber`: that one is built around
 * playback (`isPlaying`, `playbackSpeed`, play/pause/skip) and belongs to
 * `/replay`. Navigation and playback are different jobs, and passing no-op
 * playback handlers to borrow the band would put dead controls on a screen
 * that cannot play anything. The band geometry below matches the scrubber's on
 * purpose so the two surfaces read as the same timeline.
 */
export interface SegmentNavigatorProps {
  segments: AppSegment[]
  /** Currently drilled-into segment, matched by its `start` timestamp. */
  selectedStart: string | null
  onSelect: (segment: AppSegment | null) => void
}

interface Placed {
  segment: AppSegment
  leftPct: number
  widthPct: number
}

export function SegmentNavigator({ segments, selectedStart, onSelect }: SegmentNavigatorProps) {
  const { t, i18n } = useTranslation()
  // #9812: some hosts mount this without a full i18n instance (the reauth
  // journey test does, and so does any consumer that stubs `useTranslation`).
  // Reading `locale` unguarded threw there and took the whole timeline
  // down — a navigation aid must not be able to break the page it sits on.
  const locale = i18n?.language ?? 'en'

  const { placed, rangeLabel } = useMemo(() => {
    if (segments.length === 0) return { placed: [] as Placed[], rangeLabel: '' }

    const starts = segments.map((s) => new Date(s.start).getTime())
    const ends = segments.map((s) => new Date(s.end).getTime())
    const from = Math.min(...starts)
    const to = Math.max(...ends)
    const span = to - from

    const items: Placed[] = segments.map((segment) => {
      const segStart = new Date(segment.start).getTime()
      const segEnd = new Date(segment.end).getTime()
      return {
        segment,
        leftPct: span > 0 ? ((segStart - from) / span) * 100 : 0,
        // Floor the width so a very short segment stays clickable rather than
        // collapsing to an invisible sliver — the same 0.5% floor the replay
        // scrubber uses.
        widthPct: span > 0 ? Math.max(((segEnd - segStart) / span) * 100, 0.5) : 100,
      }
    })

    const fmt = (ms: number) => new Date(ms).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' })
    return { placed: items, rangeLabel: span > 0 ? `${fmt(from)} – ${fmt(to)}` : fmt(from) }
  }, [segments, locale])

  const handleSelect = useCallback(
    (segment: AppSegment) => {
      // Clicking the active segment clears the drill-down. Without this the
      // only way back to the full range is the date picker, which reads as
      // "the filter is stuck".
      onSelect(segment.start === selectedStart ? null : segment)
    },
    [onSelect, selectedStart],
  )

  if (segments.length === 0) {
    return (
      <div
        data-testid="segment-navigator-empty"
        className="rounded-lg border border-muted bg-surface-overlay p-4 text-center"
      >
        <p className={`${typography.small} text-content-secondary`}>
          {t(
            'timeline.segments.empty',
            'No activity segments in this range. Segments are written on a periodic checkpoint, so a very recent range can still be empty.',
          )}
        </p>
      </div>
    )
  }

  return (
    <div data-testid="segment-navigator" className="rounded-lg border border-muted bg-surface-overlay p-4 shadow">
      <div className="mb-2 flex items-center justify-between">
        <span className={`${typography.small} text-content-secondary`}>
          {t('timeline.segments.title', 'What was on screen')}
        </span>
        <span className={`${typography.family.mono} text-content-tertiary text-xs`}>{rangeLabel}</span>
      </div>

      <fieldset
        // Not a slider: this selects a discrete segment rather than scrubbing a
        // continuous position, so each band is its own button and keyboard
        // users get tab + Enter instead of arrow-key seeking.
        className="relative flex h-10 w-full min-w-0 overflow-hidden rounded-lg border-0 bg-hover p-0"
        aria-label={t('timeline.segments.axis', 'Activity segments')}
      >
        {placed.map(({ segment, leftPct, widthPct }) => {
          const active = segment.start === selectedStart
          return (
            <button
              type="button"
              key={`seg-${segment.app_name}-${segment.start}`}
              data-testid="segment-band"
              data-segment-start={segment.start}
              aria-pressed={active}
              onClick={() => handleSelect(segment)}
              title={`${segment.app_name} · ${new Date(segment.start).toLocaleTimeString(locale)}`}
              className={`absolute top-0 h-full ${motion.opacity} hover:opacity-80 ${
                active ? 'ring-2 ring-brand ring-inset' : ''
              }`}
              style={{
                left: `${leftPct}%`,
                width: `${widthPct}%`,
                backgroundColor: segment.color,
              }}
            >
              <span className="sr-only">{segment.app_name}</span>
            </button>
          )
        })}
      </fieldset>

      <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1">
        {Array.from(new Set(segments.map((s) => s.app_name))).map((appName) => {
          const color = segments.find((s) => s.app_name === appName)?.color || palette.gray500
          return (
            <span key={appName} className="flex items-center gap-1.5">
              <span className="h-2 w-2 rounded-full" style={{ backgroundColor: color }} />
              <span className={`${typography.small} text-content-secondary`}>{appName}</span>
            </span>
          )
        })}
      </div>

      {selectedStart && (
        <p className={`mt-2 ${typography.small} text-content-tertiary`}>
          {t(
            'timeline.segments.filtered',
            'Showing frames from the selected segment. Click it again to see the whole range.',
          )}
        </p>
      )}
    </div>
  )
}

export default SegmentNavigator
