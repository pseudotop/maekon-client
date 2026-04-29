import { useTranslation } from 'react-i18next'
import { motion } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface SnoozeOption {
  i18nKey: string
  fallback: string
  minutes: number
}

const options: SnoozeOption[] = [
  { i18nKey: 'suggestions.snooze30min', fallback: '30 minutes', minutes: 30 },
  { i18nKey: 'suggestions.snooze1hr', fallback: '1 hour', minutes: 60 },
  { i18nKey: 'suggestions.snooze2hr', fallback: '2 hours', minutes: 120 },
  { i18nKey: 'suggestions.snooze4hr', fallback: '4 hours', minutes: 240 },
  { i18nKey: 'suggestions.snoozeTomorrow', fallback: 'Tomorrow 9 AM', minutes: 0 }, // calculated at runtime
]

interface SnoozePopoverProps {
  onSelect: (minutes: number) => void
  onCancel: () => void
}

function minutesToTomorrow9AM(): number {
  const now = new Date()
  const tomorrow = new Date(now)
  tomorrow.setDate(tomorrow.getDate() + 1)
  tomorrow.setHours(9, 0, 0, 0)
  return Math.ceil((tomorrow.getTime() - now.getTime()) / 60000)
}

export function SnoozePopover({ onSelect, onCancel }: SnoozePopoverProps) {
  const { t } = useTranslation()

  return (
    <div
      className={cn(
        'absolute right-0 bottom-full z-10 mb-1',
        'rounded-lg bg-surface-sunken/95 shadow-lg backdrop-blur-md',
        'min-w-[140px] border border-border-default p-1',
      )}
    >
      {options.map((opt) => (
        <button
          key={opt.i18nKey}
          type="button"
          className={cn(
            'w-full rounded px-3 py-1.5 text-left text-content-primary text-xs hover:bg-content-inverse/10',
            motion.colors,
          )}
          onClick={() => onSelect(opt.minutes === 0 ? minutesToTomorrow9AM() : opt.minutes)}
        >
          {t(opt.i18nKey, opt.fallback)}
        </button>
      ))}
      <button
        type="button"
        className={cn(
          'mt-0.5 w-full rounded border-border-default border-t px-3 py-1.5 text-left text-content-secondary text-xs hover:bg-content-inverse/10',
          motion.colors,
        )}
        onClick={onCancel}
      >
        {t('suggestions.snoozeCancel', 'Cancel')}
      </button>
    </div>
  )
}
