/**
 * TimelineView — vertical timetable with colored regime blocks.
 * Pure CSS layout (no Recharts), proportional block heights based on duration.
 * Supports inline recalibration via context menu on each segment.
 */
import { Settings2 } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { CreateOverrideRequest, RegimeOverride } from '../api/contracts'
import { colors, motion, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import SegmentContextMenu from './SegmentContextMenu'
import { Badge, Card, Spinner } from './ui'

interface TimelineEntry {
  segment_id: string
  start_time: string
  end_time: string
  duration_mins: number
  regime_label: string
  regime_color: string
  regime_id?: string
  dominant_app: string
  content_summary: Array<{ content: string; work_type: string; mins: number }>
  annotation?: { highlight_type: string; text: string }
}

interface RegimeOption {
  id: string
  label: string
}

interface TimelineViewProps {
  timeline: TimelineEntry[]
  onSegmentClick?: (segmentId: string) => void
  overrides?: RegimeOverride[]
  regimeOptions?: RegimeOption[]
  onCreateOverride?: (req: CreateOverrideRequest) => void
  isMutating?: boolean
}

const annotationIcons: Record<string, string> = {
  ACHIEVEMENT: '\u{1F3C6}',
  WARNING: '\u{26A0}\u{FE0F}',
  SUGGESTION: '\u{1F4A1}',
}

function formatTime(iso: string): string {
  try {
    const date = new Date(iso)
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false })
  } catch {
    return iso.slice(11, 16)
  }
}

export default function TimelineView({
  timeline,
  onSegmentClick,
  overrides,
  regimeOptions,
  onCreateOverride,
  isMutating,
}: TimelineViewProps) {
  const { t } = useTranslation()
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [menuSegmentId, setMenuSegmentId] = useState<string | null>(null)

  // Build override lookup map
  const overrideMap = useMemo(() => {
    const map = new Map<string, RegimeOverride>()
    if (overrides) {
      for (const o of overrides) {
        map.set(o.segment_id, o)
      }
    }
    return map
  }, [overrides])

  if (timeline.length === 0) {
    return (
      <Card padding="md">
        <p className={cn(typography.body, colors.text.secondary, 'py-8 text-center')}>
          No activity recorded for this day
        </p>
      </Card>
    )
  }

  const handleClick = (segmentId: string) => {
    setExpandedId((prev) => (prev === segmentId ? null : segmentId))
    onSegmentClick?.(segmentId)
  }

  const handleGearClick = (e: React.MouseEvent, segmentId: string) => {
    e.stopPropagation()
    setMenuSegmentId((prev) => (prev === segmentId ? null : segmentId))
  }

  const handleMarkAsNoise = (segmentId: string) => {
    const entry = timeline.find((e) => e.segment_id === segmentId)
    onCreateOverride?.({
      segment_id: segmentId,
      original_regime_id: entry?.regime_id,
      action: {
        type: 'MARK_AS_PERSONAL_TIME',
        from: entry?.start_time ?? '',
        to: entry?.end_time ?? '',
      },
    })
  }

  const handleReassignRegime = (segmentId: string, targetRegimeId: string) => {
    const entry = timeline.find((e) => e.segment_id === segmentId)
    onCreateOverride?.({
      segment_id: segmentId,
      original_regime_id: entry?.regime_id,
      action: { type: 'REASSIGN_REGIME', target_regime_id: targetRegimeId },
    })
  }

  function getOverrideLabel(override: RegimeOverride): string {
    switch (override.user_action.type) {
      case 'MARK_AS_PERSONAL_TIME':
        return t('recalibration.personalTime')
      case 'MARK_AS_NOISE':
        return t('recalibration.personalTime')
      case 'REASSIGN_REGIME': {
        const action = override.user_action as { type: 'REASSIGN_REGIME'; target_regime_id: string }
        const regime = regimeOptions?.find((r) => r.id === action.target_regime_id)
        return regime?.label ?? action.target_regime_id
      }
      default:
        return t('recalibration.overridden')
    }
  }

  // Scale: 1 min = 2px, minimum 32px per block
  const minBlockHeight = 32

  return (
    <Card padding="md">
      <div className="space-y-0">
        {timeline.map((entry) => {
          const blockHeight = Math.max((entry.duration_mins ?? 0) * 2, minBlockHeight)
          const isExpanded = expandedId === entry.segment_id
          const override = overrideMap.get(entry.segment_id)
          const isOverridden = !!override
          const isMenuOpen = menuSegmentId === entry.segment_id

          return (
            <div key={entry.segment_id} className="flex gap-3">
              {/* Time label column */}
              <div className="flex w-14 flex-shrink-0 flex-col items-end pt-2">
                <span className={cn(typography.weight.medium, 'text-xs', colors.text.secondary)}>
                  {formatTime(entry.start_time)}
                </span>
              </div>

              {/* Colored block column */}
              <div className="group relative flex-1">
                <button
                  type="button"
                  className={cn(
                    `w-full rounded-lg border-l-4 px-3 py-2 text-left ${motion.colors} hover:opacity-90`,
                    isExpanded ? 'ring-2 ring-brand-signal/50' : '',
                    isOverridden ? 'opacity-80' : '',
                  )}
                  style={{
                    minHeight: `${blockHeight}px`,
                    borderLeftColor: entry.regime_color,
                    backgroundColor: `${entry.regime_color}15`,
                  }}
                  onClick={() => handleClick(entry.segment_id)}
                  aria-label={`${entry.regime_label} ${entry.duration_mins} minutes - click to expand`}
                  aria-expanded={isExpanded}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span
                      className={cn(
                        typography.weight.semibold,
                        'text-xs',
                        colors.text.primary,
                        isOverridden ? 'line-through' : '',
                      )}
                    >
                      {entry.regime_label}
                    </span>
                    <div className="flex items-center gap-1.5">
                      {isOverridden && (
                        <Badge color="warning" size="sm">
                          {getOverrideLabel(override)}
                        </Badge>
                      )}
                      <span className={cn('text-xs', colors.text.tertiary)}>{entry.duration_mins}m</span>
                    </div>
                  </div>
                  <span className={cn('text-xs', colors.text.secondary)}>{entry.dominant_app}</span>

                  {/* Content summary (top items) */}
                  {entry.content_summary.length > 0 && (
                    <div className="mt-1 space-y-0.5">
                      {entry.content_summary.slice(0, isExpanded ? undefined : 2).map((item) => (
                        <div
                          key={`${entry.segment_id}-${item.work_type}-${item.mins}`}
                          className="flex items-center gap-1.5"
                        >
                          <span className={cn('text-xs', colors.text.tertiary)}>
                            {item.work_type} ({item.mins}m)
                          </span>
                          <span className={cn('truncate text-xs', colors.text.secondary)}>{item.content}</span>
                        </div>
                      ))}
                    </div>
                  )}

                  {/* Annotation bubble */}
                  {entry.annotation && (
                    <div className="mt-2 inline-flex items-center gap-1 rounded-md bg-surface-muted px-2 py-1">
                      <span aria-hidden="true">{annotationIcons[entry.annotation.highlight_type] ?? '\u{1F4A1}'}</span>
                      <span className={cn('text-xs', colors.text.secondary)}>{entry.annotation.text}</span>
                    </div>
                  )}
                </button>

                {/* Gear icon for recalibration — visible on hover */}
                {onCreateOverride && regimeOptions && (
                  <button
                    type="button"
                    className={cn(
                      `absolute top-2 right-2 rounded p-1 opacity-0 ${motion.opacity} group-hover:opacity-100`,
                      'hover:bg-surface-muted',
                      isMenuOpen ? 'opacity-100' : '',
                    )}
                    onClick={(e) => handleGearClick(e, entry.segment_id)}
                    aria-label={t('recalibration.changeRegimeTo')}
                    aria-haspopup="menu"
                    aria-expanded={isMenuOpen}
                  >
                    {isMutating ? <Spinner size="sm" /> : <Settings2 className="h-3.5 w-3.5 text-content-muted" />}
                  </button>
                )}

                {/* Context menu */}
                {isMenuOpen && regimeOptions && (
                  <SegmentContextMenu
                    segmentId={entry.segment_id}
                    currentRegimeId={entry.regime_id ?? ''}
                    regimeOptions={regimeOptions}
                    onMarkAsNoise={handleMarkAsNoise}
                    onReassignRegime={handleReassignRegime}
                    onClose={() => setMenuSegmentId(null)}
                  />
                )}
              </div>
            </div>
          )
        })}
      </div>
    </Card>
  )
}

TimelineView.displayName = 'TimelineView'
