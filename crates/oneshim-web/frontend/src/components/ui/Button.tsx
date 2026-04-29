/**
 *
 */
import { forwardRef } from 'react'
import { interaction, radius } from '../../styles/tokens'
import { buttonVariants } from '../../styles/variants'
import { cn } from '../../utils/cn'
import { Spinner } from './Spinner'

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: keyof typeof buttonVariants.variant
  size?: keyof typeof buttonVariants.size
  isLoading?: boolean
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = 'primary', size = 'md', isLoading, disabled, children, ...props }, ref) => {
    return (
      <button
        ref={ref}
        className={cn(
          'inline-flex items-center justify-center',
          radius.md,
          interaction.interactive,
          interaction.focusRing,
          interaction.disabled,
          buttonVariants.variant[variant],
          buttonVariants.size[size],
          className,
        )}
        disabled={disabled || isLoading}
        {...props}
      >
        {isLoading && <Spinner className="mr-2" size="sm" />}
        {children}
      </button>
    )
  },
)

Button.displayName = 'Button'
