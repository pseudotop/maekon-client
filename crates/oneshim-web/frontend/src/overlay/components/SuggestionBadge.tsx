import { useTranslation } from 'react-i18next'
import { motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface SuggestionBadgeProps {
  count: number
  onClick: () => void
}

export function SuggestionBadge({ count, onClick }: SuggestionBadgeProps) {
  const { t } = useTranslation()
  if (count === 0) return null

  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'fixed top-4 right-4 z-50',
        'flex items-center gap-1.5 rounded-full px-3 py-1.5',
        `bg-brand text-content-inverse text-xs ${typography.weight.medium}`,
        'cursor-pointer shadow-lg',
        `hover:bg-brand/90 ${motion.colors}`,
        'animate-pulse',
      )}
    >
      <span>{count}</span>
      <span>{t('suggestions.badgeNew', 'new')}</span>
    </button>
  )
}
