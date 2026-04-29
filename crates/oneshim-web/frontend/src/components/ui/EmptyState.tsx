import { type ReactNode, useId } from 'react'
import { typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { Button } from './Button'

export interface EmptyStateProps {
  icon: ReactNode
  title: string
  description: string
  action?: {
    label: string
    onClick: () => void
  }
}

export function EmptyState({ icon, title, description, action }: EmptyStateProps) {
  const headingId = useId()
  return (
    <section aria-labelledby={headingId} className="flex flex-col items-center justify-center px-6 py-16">
      <div className="mb-4 flex h-16 w-16 items-center justify-center rounded-full bg-surface-elevated text-content-muted">
        {icon}
      </div>
      <h3 id={headingId} className={cn('mb-2 text-content', typography.h3)}>
        {title}
      </h3>
      <p className={cn('mb-4 max-w-md text-center text-content-secondary', typography.body)}>{description}</p>
      {action && (
        <Button type="button" variant="primary" size="md" onClick={action.onClick}>
          {action.label}
        </Button>
      )}
    </section>
  )
}
