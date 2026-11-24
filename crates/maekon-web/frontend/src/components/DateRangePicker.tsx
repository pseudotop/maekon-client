import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { motion } from '../styles/tokens'
import { cn } from '../utils/cn'
import {
  formatLocalCalendarDate,
  localCalendarDateFromValue,
  localDayBoundaryIso,
  shiftLocalCalendarDate,
} from '../utils/localDate'
import { Input } from './ui'

interface DateRangePickerProps {
  onRangeChange: (from: string | undefined, to: string | undefined) => void
  initialFrom?: string
  initialTo?: string
  initialPreset?: PresetRange
}

type PresetRange = 'today' | '7days' | '30days' | 'custom'

function getToday() {
  return formatLocalCalendarDate(new Date())
}

function inferInitialPreset(initialFrom?: string, initialTo?: string): PresetRange {
  if (!initialFrom && !initialTo) {
    return 'today'
  }

  const today = getToday()
  const weekStart = shiftLocalCalendarDate(today, -7)
  const monthStart = shiftLocalCalendarDate(today, -30)

  const fromDate = localCalendarDateFromValue(initialFrom)
  const toDate = localCalendarDateFromValue(initialTo)

  if (fromDate === today && toDate === today) {
    return 'today'
  }

  if (fromDate === weekStart && toDate === today) {
    return '7days'
  }

  if (fromDate === monthStart && toDate === today) {
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
  const [customFrom, setCustomFrom] = useState(() => localCalendarDateFromValue(initialFrom))
  const [customTo, setCustomTo] = useState(() => localCalendarDateFromValue(initialTo))

  // Stable ref prevents callback identity changes from re-triggering the effect
  const onRangeChangeRef = useRef(onRangeChange)
  onRangeChangeRef.current = onRangeChange

  useEffect(() => {
    let from: string | undefined
    let to: string | undefined
    const today = getToday()

    switch (preset) {
      case 'today':
        from = localDayBoundaryIso(today, 'start')
        to = localDayBoundaryIso(today, 'end')
        break
      case '7days':
        from = localDayBoundaryIso(shiftLocalCalendarDate(today, -7), 'start')
        to = localDayBoundaryIso(today, 'end')
        break
      case '30days':
        from = localDayBoundaryIso(shiftLocalCalendarDate(today, -30), 'start')
        to = localDayBoundaryIso(today, 'end')
        break
      case 'custom':
        if (customFrom && customTo) {
          from = localDayBoundaryIso(customFrom, 'start')
          to = localDayBoundaryIso(customTo, 'end')
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
          <Input
            id="date-from"
            type="date"
            inputSize="sm"
            value={customFrom}
            onChange={(e) => setCustomFrom(e.target.value)}
            className="w-auto"
          />
          <span className="text-content-muted" aria-hidden="true">
            ~
          </span>
          <label className="sr-only" htmlFor="date-to">
            {t('dateRange.to', 'To')}
          </label>
          <Input
            id="date-to"
            type="date"
            inputSize="sm"
            value={customTo}
            onChange={(e) => setCustomTo(e.target.value)}
            className="w-auto"
          />
        </div>
      )}
    </div>
  )
}
