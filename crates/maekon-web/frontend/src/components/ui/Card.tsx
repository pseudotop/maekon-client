/**
 *
 */
import { forwardRef } from 'react'
import { colors, motion, radius, typography } from '../../styles/tokens'
import { cardVariants } from '../../styles/variants'
import { cn } from '../../utils/cn'

export interface CardProps extends React.HTMLAttributes<HTMLDivElement> {
  variant?: keyof typeof cardVariants.variant
  padding?: keyof typeof cardVariants.padding
}

export const Card = forwardRef<HTMLDivElement, CardProps>(
  ({ className, variant = 'default', padding = 'md', ...props }, ref) => {
    return (
      <div
        ref={ref}
        className={cn(
          radius.md,
          motion.colors,
          cardVariants.variant[variant],
          cardVariants.padding[padding],
          className,
        )}
        {...props}
      />
    )
  },
)

Card.displayName = 'Card'

export function CardHeader({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('mb-4', className)} {...props} />
}

export interface CardTitleProps extends React.HTMLAttributes<HTMLHeadingElement> {
  sticky?: boolean
}

export function CardTitle({ className, sticky, ...props }: CardTitleProps) {
  return (
    <h2
      className={cn(
        typography.h3,
        colors.text.primary,
        sticky && 'sticky top-0 z-10 -mx-6 -mt-6 mb-4 rounded-t-md bg-surface-elevated px-6 pt-6 pb-3',
        className,
      )}
      {...props}
    />
  )
}

export function CardContent({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn(className)} {...props} />
}
