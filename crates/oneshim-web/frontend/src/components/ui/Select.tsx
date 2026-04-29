/**
 *
 */
import { forwardRef } from 'react'
import { colors, interaction, radius } from '../../styles/tokens'
import { selectVariants } from '../../styles/variants'
import { cn } from '../../utils/cn'

export interface SelectProps extends React.SelectHTMLAttributes<HTMLSelectElement> {
  variant?: keyof typeof selectVariants.variant
  selectSize?: keyof typeof selectVariants.size
}

export const Select = forwardRef<HTMLSelectElement, SelectProps>(
  ({ className, variant = 'default', selectSize = 'md', children, ...props }, ref) => {
    return (
      <select
        ref={ref}
        className={cn(
          'w-full border',
          radius.md,
          colors.text.primary,
          interaction.interactive,
          interaction.focusRing,
          interaction.disabled,
          'disabled:border-muted disabled:bg-surface-muted disabled:text-content-tertiary disabled:shadow-none',
          selectVariants.variant[variant],
          selectVariants.size[selectSize],
          className,
        )}
        {...props}
      >
        {children}
      </select>
    )
  },
)

Select.displayName = 'Select'
