/**
 *
 */
import { useTranslation } from 'react-i18next'
import { iconSize, interaction, motion, typography } from '../styles/tokens'
import { tagBadgeVariants } from '../styles/variants'
import { cn } from '../utils/cn'

interface TagBadgeProps {
  name: string
  color: string
  onRemove?: () => void
  onClick?: () => void
  selected?: boolean
  size?: keyof typeof tagBadgeVariants.size
}

export function TagBadge({ name, color, onRemove, onClick, selected = false, size = 'md' }: TagBadgeProps) {
  const { t } = useTranslation()
  const bgColor = `${color}20`
  const borderColor = selected ? color : `${color}60`

  const sharedClassName = cn(
    `inline-flex items-center gap-1 rounded-full border ${typography.weight.medium} ${motion.all}`,
    tagBadgeVariants.size[size],
    onClick && 'cursor-pointer hover:opacity-80',
    selected && 'ring-2 ring-offset-1',
  )

  const sharedStyle = {
    backgroundColor: bgColor,
    borderColor: borderColor,
    color: color,
    ...(selected && { ringColor: color }),
  }

  const children = (
    <>
      <span className="h-2 w-2 rounded-full" style={{ backgroundColor: color }} />
      {name}
      {onRemove && (
        <button
          type="button"
          className={cn('ml-0.5 rounded-sm hover:opacity-70', interaction.focusRing)}
          onClick={(e) => {
            e.stopPropagation()
            onRemove()
          }}
          aria-label={t('timeline.removeTag', { name, defaultValue: 'Remove {{name}} tag' })}
        >
          <svg className={iconSize.xs} fill="none" viewBox="0 0 24 24" stroke="currentColor" aria-hidden="true">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      )}
    </>
  )

  if (onClick) {
    return (
      <button type="button" className={sharedClassName} style={sharedStyle} onClick={onClick}>
        {children}
      </button>
    )
  }

  return (
    <span className={sharedClassName} style={sharedStyle}>
      {children}
    </span>
  )
}

import { palette } from '../styles/tokens'

export const TAG_COLORS = [
  palette.blue500,
  palette.red500,
  palette.green500,
  palette.amber500,
  palette.violet500,
  palette.pink500,
  palette.teal500,
  palette.orange500,
  palette.indigo500,
  palette.lime500,
]

export function getRandomTagColor(): string {
  return TAG_COLORS[Math.floor(Math.random() * TAG_COLORS.length)]
}
