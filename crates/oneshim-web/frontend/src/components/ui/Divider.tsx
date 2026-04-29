/**
 *
 */
import { forwardRef } from 'react'
import { cn } from '../../utils/cn'

export interface DividerProps extends React.HTMLAttributes<HTMLHRElement> {
  orientation?: 'horizontal' | 'vertical'
}

export const Divider = forwardRef<HTMLHRElement, DividerProps>(
  ({ className, orientation = 'horizontal', ...props }, ref) => {
    return (
      <hr
        ref={ref}
        className={cn(
          'border-0 border-DEFAULT',
          orientation === 'horizontal' ? 'w-full border-t' : 'h-full border-l',
          className,
        )}
        {...props}
      />
    )
  },
)

Divider.displayName = 'Divider'
