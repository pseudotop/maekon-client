/**
 *
 */
import type { ReactNode } from 'react'
import { forwardRef } from 'react'
import { iconSize, radius, typography } from '../../styles/tokens'
import { alertVariants } from '../../styles/variants'
import { cn } from '../../utils/cn'

export interface AlertProps extends React.HTMLAttributes<HTMLDivElement> {
  variant?: keyof typeof alertVariants.variant
  title?: string
  icon?: ReactNode
}

export const Alert = forwardRef<HTMLDivElement, AlertProps>(
  ({ className, variant = 'default', title, icon, children, ...props }, ref) => {
    const semanticRole = variant === 'error' || variant === 'warning' ? 'alert' : 'status'

    return (
      <div
        ref={ref}
        role={semanticRole}
        className={cn(radius.md, 'p-4', alertVariants.variant[variant], className)}
        {...props}
      >
        <div className="flex gap-3">
          {icon && <div className={cn(iconSize.md, 'mt-0.5 shrink-0', alertVariants.iconColor[variant])}>{icon}</div>}
          <div className="min-w-0 flex-1">
            {title && <p className={cn(typography.label, 'mb-1 text-content')}>{title}</p>}
            <div className={cn(typography.body, 'text-content-secondary')}>{children}</div>
          </div>
        </div>
      </div>
    )
  },
)

Alert.displayName = 'Alert'
