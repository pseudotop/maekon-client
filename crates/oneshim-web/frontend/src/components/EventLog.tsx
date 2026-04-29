import { AppWindow, ArrowRightLeft, Camera, Monitor, Moon } from 'lucide-react'
import { memo, type Ref, useCallback, useEffect, useMemo, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import type { TimelineItem } from '../api/client'
import { iconSize, motion, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { formatTime } from '../utils/formatters'

interface EventLogProps {
  items: TimelineItem[]
  currentTime: Date
  onItemClick: (time: Date) => void
}

interface EventLogItemProps {
  item: TimelineItem
  isActive: boolean
  activeRef: Ref<HTMLButtonElement> | undefined
  captureLabel: string
  idleLabel: string
  minLabel: string
  onItemClick: (time: Date) => void
}

function getEventIcon(item: TimelineItem) {
  if (item.type === 'Frame') {
    return <Camera className={cn(iconSize.base, 'text-brand-text')} />
  }
  if (item.type === 'IdlePeriod') {
    return <Moon className={cn(iconSize.base, 'text-content-muted')} />
  }
  const eventType = item.event_type.toLowerCase()
  if (eventType.includes('appswitch') || eventType.includes('context')) {
    return <ArrowRightLeft className={cn(iconSize.base, 'text-semantic-info')} />
  }
  if (eventType.includes('window')) {
    return <AppWindow className={cn(iconSize.base, 'text-brand-text')} />
  }
  return <Monitor className={cn(iconSize.base, 'text-semantic-warning')} />
}

function getEventLabel(item: TimelineItem, captureLabel: string, idleLabel: string, minLabel: string) {
  if (item.type === 'Frame') {
    return captureLabel
  }
  if (item.type === 'IdlePeriod') {
    const mins = Math.round(item.duration_secs / 60)
    return `${idleLabel} (${mins}${minLabel})`
  }
  return item.event_type
}

function getItemTime(item: TimelineItem): Date {
  if (item.type === 'IdlePeriod') {
    return new Date(item.start)
  }
  return new Date(item.timestamp)
}

function getItemKey(item: TimelineItem): string {
  if (item.type === 'IdlePeriod') return `idle-${item.start}`
  if (item.type === 'Frame') return `frame-${item.id}`
  return `event-${item.timestamp}-${item.event_type}`
}

const EventLogItem = memo(function EventLogItem({
  item,
  isActive,
  activeRef,
  captureLabel,
  idleLabel,
  minLabel,
  onItemClick,
}: EventLogItemProps) {
  const itemTime = getItemTime(item)
  const timeStr = item.type === 'IdlePeriod' ? formatTime(item.start) : formatTime(item.timestamp)

  return (
    <button
      type="button"
      ref={activeRef}
      className={cn(
        'w-full cursor-pointer px-4 py-2 text-left',
        motion.colors,
        isActive ? 'border-brand-signal border-l-2 bg-brand-signal/5' : 'border-transparent border-l-2 hover:bg-hover',
      )}
      onClick={() => onItemClick(itemTime)}
    >
      <div className="flex items-start space-x-3">
        {/* UI note */}
        <div className="mt-0.5">{getEventIcon(item)}</div>

        {/* UI note */}
        <div className="min-w-0 flex-1">
          {/* UI note */}
          <div className="flex items-center justify-between">
            <span className={cn(typography.mono, 'text-content-secondary', typography.caption)}>{timeStr}</span>
            <span
              className={cn(
                'rounded px-1.5 py-0.5',
                typography.caption,
                item.type === 'Frame'
                  ? 'bg-brand-signal/10 text-brand-text'
                  : item.type === 'IdlePeriod'
                    ? 'bg-surface-elevated text-content-secondary'
                    : 'bg-semantic-info/10 text-semantic-info',
              )}
            >
              {getEventLabel(item, captureLabel, idleLabel, minLabel)}
            </span>
          </div>

          {/* UI note */}
          {item.type !== 'IdlePeriod' && (
            <div className="mt-1">
              {item.type === 'Frame' ? (
                <>
                  <p className={cn('truncate text-content', typography.label)}>{item.app_name}</p>
                  <p className={cn('truncate text-content-secondary', typography.caption)}>{item.window_title}</p>
                </>
              ) : (
                item.app_name && (
                  <>
                    <p className={cn('truncate text-content', typography.label)}>{item.app_name}</p>
                    {item.window_title && (
                      <p className={cn('truncate text-content-secondary', typography.caption)}>{item.window_title}</p>
                    )}
                  </>
                )
              )}
            </div>
          )}

          {/* UI note */}
          {item.type === 'Frame' && (
            <div className="mt-1 flex items-center space-x-2">
              <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-surface-muted">
                <div
                  className={cn(
                    'h-full',
                    item.importance >= 0.7
                      ? 'bg-semantic-success'
                      : item.importance >= 0.4
                        ? 'bg-semantic-warning'
                        : 'bg-surface-muted',
                  )}
                  style={{ width: `${(item.importance ?? 0) * 100}%` }}
                />
              </div>
              <span className={cn('text-content-secondary', typography.caption)}>
                {Math.round((item.importance ?? 0) * 100)}%
              </span>
            </div>
          )}
        </div>
      </div>
    </button>
  )
})

export default memo(function EventLog({ items, currentTime, onItemClick }: EventLogProps) {
  const { t } = useTranslation()
  const activeItemRef = useRef<HTMLButtonElement>(null)

  const captureLabel = t('replay.capture', 'Capture')
  const idleLabel = t('replay.idle', 'Idle')
  const minLabel = t('dashboard.minutes', 'min')

  const activeIndex = useMemo(() => {
    if (items.length === 0) return -1
    const currentMs = currentTime.getTime()

    let closestIndex = 0
    let closestDiff = Infinity

    for (let i = 0; i < items.length; i++) {
      const itemTime = getItemTime(items[i]).getTime()
      const diff = Math.abs(itemTime - currentMs)

      if (itemTime <= currentMs && diff < closestDiff) {
        closestDiff = diff
        closestIndex = i
      }
    }

    return closestIndex
  }, [items, currentTime])

  // biome-ignore lint/correctness/useExhaustiveDependencies: activeIndex change updates activeItemRef via render
  useEffect(() => {
    if (activeItemRef.current) {
      activeItemRef.current.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
    }
  }, [activeIndex])

  const handleItemClick = useCallback(
    (time: Date) => {
      onItemClick(time)
    },
    [onItemClick],
  )

  return (
    <div className="flex h-full flex-col rounded-lg border border-muted bg-surface-overlay shadow">
      {/* UI note */}
      <div className="border-muted border-b px-4 py-3">
        <h3 className={cn(typography.label, 'text-content')}>{t('replay.eventLog', 'Event Log')}</h3>
        <p className={cn('mt-0.5 text-content-secondary', typography.caption)}>
          {items.length}
          {t('replay.items', ' items')}
        </p>
      </div>

      {/* event list */}
      <div className="flex-1 overflow-y-auto">
        {items.length === 0 ? (
          <div className={cn('flex h-32 items-center justify-center text-content-secondary', typography.body)}>
            {t('common.noData', 'No data')}
          </div>
        ) : (
          <div className="divide-y divide-border">
            {items.map((item, index) => {
              const isActive = index === activeIndex
              return (
                <EventLogItem
                  key={getItemKey(item)}
                  item={item}
                  isActive={isActive}
                  activeRef={isActive ? activeItemRef : undefined}
                  captureLabel={captureLabel}
                  idleLabel={idleLabel}
                  minLabel={minLabel}
                  onItemClick={handleItemClick}
                />
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
})
