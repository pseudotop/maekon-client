/**
 *
 */
import { forwardRef } from 'react'
import { colors, interaction, radius } from '../../styles/tokens'
import { inputVariants } from '../../styles/variants'
import { cn } from '../../utils/cn'

export interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  inputSize?: keyof typeof inputVariants.size
  error?: boolean
}

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ className, inputSize = 'md', error, ...props }, ref) => {
    return (
      <input
        ref={ref}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        spellCheck={false}
        aria-invalid={error || undefined}
        className={cn(
          'w-full border placeholder-content-tertiary',
          radius.md,
          colors.text.primary,
          interaction.interactive,
          interaction.focusRing,
          inputVariants.size[inputSize],
          error ? inputVariants.variant.error : inputVariants.variant.default,
          className,
        )}
        {...props}
      />
    )
  },
)

Input.displayName = 'Input'
