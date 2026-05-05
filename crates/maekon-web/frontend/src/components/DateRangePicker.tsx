import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { interaction, motion } from '../styles/tokens'
import { cn } from '../utils/cn'

interface DateRangePickerProps {
  onRangeChange: (from: string | undefined, to: string | undefined) => void
  initialFrom?: string
  initialTo?: string
  initialPreset?: PresetRange
}

type PresetRange = 'today' | '7days' | '30days' | 'custom'

function getToday() {
  const now = new Date()
  return now.toISOString().split('T')[0]
}

function getDaysAgo(days: number) {
  const date = new Date()
  date.setDate(date.getDate() - days)
  return date.toISOString().split('T')[0]
}

function inferInitialPreset(initialFrom?: string, initialTo?: string): PresetRange {
  if (!initialFrom && !initialTo) {
    return 'today'
  }

  const today = getToday()
  const weekStart = getDaysAgo(7)
  const monthStart = getDaysAgo(30)

  if (initialFrom === today && initialTo === today) {
    return 'today'
  }

  if (initialFrom === weekStart && initialTo === today) {
    return '7days'
  }

  if (initialFrom === monthStart && initialTo === today) {
    return '30days'
  }

  return 'custom'
}

export default function DateRangePicker({
  onRangeChange,
  initialFrom,
  initialTo,
  initialPreset,
}: DateRangePickerProps) {
  const { t } = useTranslation()
  const [preset, setPreset] = useState<PresetRange>(initialPreset ?? inferInitialPreset(initialFrom, initialTo))
  const [customFrom, setCustomFrom] = useState(initialFrom || '')
  const [customTo, setCustomTo] = useState(initialTo || '')

  // Stable ref prevents callback identity changes from re-triggering the effect
  const onRangeChangeRef = useRef(onRangeChange)
  onRangeChangeRef.current = onRangeChange

  useEffect(() => {
    let from: string | undefined
    let to: string | undefined

    switch (preset) {
      case 'today':
        from = `${getToday()}T00:00:00Z`
        to = `${getToday()}T23:59:59Z`
        break
      case '7days':
        from = `${getDaysAgo(7)}T00:00:00Z`
        to = `${getToday()}T23:59:59Z`
        break
      case '30days':
        from = `${getDaysAgo(30)}T00:00:00Z`
        to = `${getToday()}T23:59:59Z`
        break
      case 'custom':
        if (customFrom && customTo) {
          from = `${customFrom}T00:00:00Z`
          to = `${customTo}T23:59:59Z`
        }
        break
    }

    onRangeChangeRef.current(from, to)
  }, [preset, customFrom, customTo])

  const handlePresetClick = (newPreset: PresetRange) => {
    setPreset(newPreset)
  }

  return (
    <div data-testid="date-range-picker" className="flex flex-wrap items-center gap-2 space-x-2">
      {/* UI note */}
      <div className="flex space-x-1">
        <button
          type="button"
          onClick={() => handlePresetClick('today')}
          className={cn(
            `rounded-lg px-3 py-1.5 text-sm ${motion.colors}`,
            preset === 'today' ? 'bg-brand text-content-inverse' : 'bg-hover text-content-strong hover:bg-active',
          )}
        >
          {t('dateRange.today')}
        </button>
        <button
          type="button"
          onClick={() => handlePresetClick('7days')}
          className={cn(
            `rounded-lg px-3 py-1.5 text-sm ${motion.colors}`,
            preset === '7days' ? 'bg-brand text-content-inverse' : 'bg-hover text-content-strong hover:bg-active',
          )}
        >
          {t('dateRange.week')}
        </button>
        <button
          type="button"
          onClick={() => handlePresetClick('30days')}
          className={cn(
            `rounded-lg px-3 py-1.5 text-sm ${motion.colors}`,
            preset === '30days' ? 'bg-brand text-content-inverse' : 'bg-hover text-content-strong hover:bg-active',
          )}
        >
          {t('dateRange.month')}
        </button>
        <button
          type="button"
          onClick={() => handlePresetClick('custom')}
          className={cn(
            `rounded-lg px-3 py-1.5 text-sm ${motion.colors}`,
            preset === 'custom' ? 'bg-brand text-content-inverse' : 'bg-hover text-content-strong hover:bg-active',
          )}
        >
          {t('dateRange.custom')}
        </button>
      </div>

      {/* UI note */}
      {preset === 'custom' && (
        <div className="flex items-center space-x-2">
          <label className="sr-only" htmlFor="date-from">
            {t('dateRange.from', 'From')}
          </label>
          <input
            id="date-from"
            type="date"
            value={customFrom}
            onChange={(e) => setCustomFrom(e.target.value)}
            className={cn(
              'rounded-lg border border-DEFAULT bg-surface-overlay px-3 py-1.5 text-content text-sm',
              interaction.focusRing,
            )}
          />
          <span className="text-content-muted" aria-hidden="true">
            ~
          </span>
          <label className="sr-only" htmlFor="date-to">
            {t('dateRange.to', 'To')}
          </label>
          <input
            id="date-to"
            type="date"
            value={customTo}
            onChange={(e) => setCustomTo(e.target.value)}
            className={cn(
              'rounded-lg border border-DEFAULT bg-surface-overlay px-3 py-1.5 text-content text-sm',
              interaction.focusRing,
            )}
          />
        </div>
      )}
    </div>
  )
}
