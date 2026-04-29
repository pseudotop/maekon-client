import { memo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { SuggestionViewDto } from '../types'
import { SnoozePopover } from './SnoozePopover'

interface SuggestionItemProps {
  item: SuggestionViewDto
  onAction: (id: string, action: 'accept' | 'reject' | 'defer' | 'explain', snoozeMinutes?: number) => void
}

const priorityClasses: Record<string, string> = {
  critical: 'bg-semantic-error/20 text-semantic-error',
  high: 'bg-semantic-warning/20 text-semantic-warning',
  medium: 'bg-brand/20 text-brand',
  low: 'bg-content-secondary/20 text-content-secondary',
}

export const SuggestionItem = memo(function SuggestionItem({ item, onAction }: SuggestionItemProps) {
  const { t } = useTranslation()
  const [showSnooze, setShowSnooze] = useState(false)
  const badgeClass = priorityClasses[item.priority] ?? priorityClasses.low

  return (
    <li
      aria-label={t('suggestions.suggestionLabel', { title: item.title })}
      className="list-none border-content-inverse/5 border-b px-4 py-3"
    >
      <div className="flex items-start justify-between gap-2">
        <span className={cn('text-content text-sm leading-tight', typography.weight.medium)}>{item.title}</span>
        <span
          className={cn(
            'inline-flex shrink-0 items-center rounded px-1.5 py-0.5 text-[10px]',
            typography.weight.semibold,
            badgeClass,
          )}
        >
          {item.priority}
        </span>
      </div>
      <p className="mt-1 line-clamp-2 text-content-secondary text-xs">{item.body}</p>
      <div className="mt-2 flex items-center gap-1.5">
        <button
          type="button"
          onClick={() => onAction(item.id, 'accept')}
          className={cn(
            'rounded-md bg-semantic-success/15 px-2 py-1 text-semantic-success text-xs hover:bg-semantic-success/25',
            motion.colors,
          )}
        >
          {t('suggestions.accept')}
        </button>
        <button
          type="button"
          onClick={() => onAction(item.id, 'reject')}
          className={cn(
            'rounded-md bg-semantic-error/15 px-2 py-1 text-semantic-error text-xs hover:bg-semantic-error/25',
            motion.colors,
          )}
        >
          {t('suggestions.reject')}
        </button>
        <div className="relative">
          <button
            type="button"
            onClick={() => setShowSnooze(!showSnooze)}
            className={cn(
              'rounded-md bg-content-inverse/10 px-2 py-1 text-content-secondary text-xs hover:bg-content-inverse/15',
              motion.colors,
            )}
          >
            {t('suggestions.later')}
          </button>
          {showSnooze && (
            <SnoozePopover
              onSelect={(minutes) => {
                onAction(item.id, 'defer', minutes)
                setShowSnooze(false)
              }}
              onCancel={() => setShowSnooze(false)}
            />
          )}
        </div>
        <button
          type="button"
          onClick={() => onAction(item.id, 'explain')}
          className={cn('rounded-md bg-brand/10 px-2 py-1 text-brand text-xs hover:bg-brand/20', motion.colors)}
        >
          {t('suggestions.explain')}
        </button>
        <span className="ml-auto text-[10px] text-content-tertiary">
          {Math.round(item.confidence_score * 100)}% &middot; {item.source}
        </span>
      </div>
    </li>
  )
})
