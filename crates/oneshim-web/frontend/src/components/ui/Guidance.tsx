import { Info } from 'lucide-react'
import type { ReactNode } from 'react'
import { useId } from 'react'
import { colors, iconSize, interaction, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { Badge } from './Badge'
import { Button, type ButtonProps } from './Button'

type GuidanceAction = {
  label: string
  onClick: () => void
  variant?: ButtonProps['variant']
}

type Tone = 'default' | 'info' | 'success' | 'warning' | 'danger' | 'muted'

const toneText: Record<Tone, string> = {
  default: 'text-content-strong',
  info: 'text-semantic-info',
  success: 'text-semantic-success',
  warning: 'text-semantic-warning',
  danger: 'text-semantic-error',
  muted: 'text-content-secondary',
}

const hintTone: Record<Exclude<Tone, 'success'>, string> = {
  default: 'text-content-secondary',
  info: 'text-semantic-info',
  warning: 'text-semantic-warning',
  danger: 'text-semantic-error',
  muted: 'text-content-tertiary',
}

export interface GuidanceItem {
  title: ReactNode
  description?: ReactNode
  icon?: ReactNode
}

export interface GuidanceEmptyStateProps extends Omit<React.HTMLAttributes<HTMLElement>, 'title'> {
  icon?: ReactNode
  title: ReactNode
  description: ReactNode
  guidance?: GuidanceItem[]
  primaryAction?: GuidanceAction
  secondaryAction?: GuidanceAction
}

export function GuidanceEmptyState({
  icon,
  title,
  description,
  guidance = [],
  primaryAction,
  secondaryAction,
  className,
  ...props
}: GuidanceEmptyStateProps) {
  const headingId = useId()

  return (
    <section
      aria-labelledby={headingId}
      className={cn('mx-auto flex max-w-4xl flex-col items-center px-6 py-12', className)}
      {...props}
    >
      {icon && (
        <div className="mb-4 flex h-16 w-16 items-center justify-center rounded-full bg-surface-elevated text-content-muted">
          {icon}
        </div>
      )}
      <h3 id={headingId} className={cn('mb-2 text-center text-content', typography.h3)}>
        {title}
      </h3>
      <p className={cn('max-w-xl text-center text-content-secondary', typography.body)}>{description}</p>
      {guidance.length > 0 && (
        <div className="mt-6 grid w-full gap-3 sm:grid-cols-3">
          {guidance.map((item, index) => (
            <article
              // biome-ignore lint/suspicious/noArrayIndexKey: guidance items are static caller-provided copy
              key={index}
              className="rounded-lg border border-muted bg-surface-elevated p-4"
            >
              <div className="mb-2 flex items-center gap-2 text-content-strong">
                {item.icon && <span className={cn(iconSize.base, 'shrink-0')}>{item.icon}</span>}
                <h4 className={cn(typography.weight.semibold, 'text-sm')}>{item.title}</h4>
              </div>
              {item.description && <p className="text-content-secondary text-xs">{item.description}</p>}
            </article>
          ))}
        </div>
      )}
      {(primaryAction || secondaryAction) && (
        <div className="mt-6 flex flex-wrap justify-center gap-2">
          {primaryAction && (
            <Button
              type="button"
              variant={primaryAction.variant ?? 'primary'}
              size="md"
              onClick={primaryAction.onClick}
            >
              {primaryAction.label}
            </Button>
          )}
          {secondaryAction && (
            <Button
              type="button"
              variant={secondaryAction.variant ?? 'secondary'}
              size="md"
              onClick={secondaryAction.onClick}
            >
              {secondaryAction.label}
            </Button>
          )}
        </div>
      )}
    </section>
  )
}

export interface GuidancePanelProps extends Omit<React.HTMLAttributes<HTMLElement>, 'title'> {
  title: ReactNode
  description?: ReactNode
  items: GuidanceItem[]
  footer?: ReactNode
  columns?: 2 | 3
}

export function GuidancePanel({
  title,
  description,
  items,
  footer,
  columns = 3,
  className,
  ...props
}: GuidancePanelProps) {
  const headingId = useId()

  return (
    <section
      aria-labelledby={headingId}
      className={cn('rounded-lg border border-muted bg-surface-muted p-4', className)}
      {...props}
    >
      <div className="max-w-3xl">
        <h2 id={headingId} className={cn(typography.h4, colors.text.primary)}>
          {title}
        </h2>
        {description && <p className="mt-1 text-content-secondary text-sm">{description}</p>}
      </div>
      <div className={cn('mt-4 grid gap-3', columns === 2 ? 'md:grid-cols-2' : 'md:grid-cols-3')}>
        {items.map((item, index) => (
          <article
            // biome-ignore lint/suspicious/noArrayIndexKey: guidance items are static caller-provided copy
            key={index}
            className="rounded-md border border-muted bg-surface-elevated/70 p-3"
          >
            <div className="flex items-start gap-2">
              {item.icon && <span className={cn(iconSize.base, 'mt-0.5 shrink-0 text-brand-text')}>{item.icon}</span>}
              <div className="min-w-0">
                <h3 className={cn(typography.weight.semibold, 'text-content-strong text-sm')}>{item.title}</h3>
                {item.description && <p className="mt-1 text-content-secondary text-xs">{item.description}</p>}
              </div>
            </div>
          </article>
        ))}
      </div>
      {footer && <p className="mt-3 border-muted border-t pt-3 text-content-secondary text-xs">{footer}</p>}
    </section>
  )
}

export interface FieldHintProps extends React.HTMLAttributes<HTMLParagraphElement> {
  tone?: Exclude<Tone, 'success'>
  icon?: ReactNode
}

export function FieldHint({ tone = 'default', icon, className, children, ...props }: FieldHintProps) {
  if (!icon) {
    return (
      <p className={cn('mt-1 text-xs', hintTone[tone], className)} {...props}>
        {children}
      </p>
    )
  }

  return (
    <p className={cn('mt-1 flex items-start gap-1.5 text-xs', hintTone[tone], className)} {...props}>
      <span className={cn(iconSize.xs, 'mt-0.5 shrink-0')}>{icon}</span>
      <span>{children}</span>
    </p>
  )
}

export interface SettingPreviewRow {
  label: ReactNode
  value: ReactNode
  tone?: Tone
}

export interface SettingPreviewProps extends Omit<React.HTMLAttributes<HTMLElement>, 'title'> {
  title: ReactNode
  description?: ReactNode
  rows: SettingPreviewRow[]
  footer?: ReactNode
}

export function SettingPreview({ title, description, rows, footer, className, ...props }: SettingPreviewProps) {
  const headingId = useId()

  return (
    <aside
      aria-labelledby={headingId}
      className={cn('rounded-lg border border-muted bg-surface-muted p-4', className)}
      {...props}
    >
      <h3 id={headingId} className={cn(typography.h4, 'text-content')}>
        {title}
      </h3>
      {description && <p className="mt-1 text-content-secondary text-xs">{description}</p>}
      <dl className="mt-3 space-y-3 text-sm">
        {rows.map((row, index) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: preview rows are static caller-provided fields
          <div key={index}>
            <dt className="text-content-secondary text-xs">{row.label}</dt>
            <dd className={cn('mt-0.5', toneText[row.tone ?? 'default'])}>{row.value}</dd>
          </div>
        ))}
      </dl>
      {footer && <p className="mt-4 border-muted border-t pt-3 text-content-secondary text-xs">{footer}</p>}
    </aside>
  )
}

export interface UnavailableFeatureCalloutProps extends Omit<React.HTMLAttributes<HTMLOutputElement>, 'title'> {
  title: ReactNode
  description: ReactNode
  reason?: ReactNode
  badgeLabel?: ReactNode
  icon?: ReactNode
  action?: GuidanceAction
}

export function UnavailableFeatureCallout({
  title,
  description,
  reason,
  badgeLabel = 'Unavailable',
  icon = <Info className={iconSize.md} aria-hidden="true" />,
  action,
  className,
  ...props
}: UnavailableFeatureCalloutProps) {
  return (
    <output
      className={cn('block rounded-lg border border-muted bg-surface-muted p-4 text-content', className)}
      {...props}
    >
      <div className="flex gap-3">
        <div className="mt-0.5 shrink-0 text-content-secondary">{icon}</div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className={cn(typography.h4, colors.text.primary)}>{title}</h3>
            <Badge color="default" size="sm">
              {badgeLabel}
            </Badge>
          </div>
          <p className="mt-1 text-content-secondary text-sm">{description}</p>
          {reason && <p className="mt-2 text-content-tertiary text-xs">{reason}</p>}
          {action && (
            <Button
              type="button"
              variant={action.variant ?? 'secondary'}
              size="sm"
              className={cn('mt-3', interaction.focusRing)}
              onClick={action.onClick}
            >
              {action.label}
            </Button>
          )}
        </div>
      </div>
    </output>
  )
}
