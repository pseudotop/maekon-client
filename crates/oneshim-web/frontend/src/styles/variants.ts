/**
 * Component variant tokens — backed by CSS custom properties via tokens.ts.
 */
import { colors } from './tokens'

export const buttonVariants = {
  variant: {
    primary: `${colors.primary.DEFAULT} ${colors.primary.hover} ${colors.text.inverse} font-medium`,
    secondary: 'bg-surface-muted hover:bg-active text-content font-medium',
    ghost: `hover:bg-hover ${colors.text.secondary}`,
    danger: 'bg-semantic-error hover:bg-semantic-error-hover text-content-inverse font-medium',
    warning: 'bg-semantic-warning hover:bg-semantic-warning-hover text-content-inverse font-medium',
  },
  size: {
    sm: 'px-3 py-1.5 text-sm',
    md: 'px-4 py-2 text-sm',
    lg: 'px-6 py-3 text-base',
    icon: 'p-2.5',
  },
} as const

export const cardVariants = {
  variant: {
    default: colors.surface.elevated,
    elevated: colors.surface.muted,
    highlight: 'bg-brand-signal/10 border border-brand-signal/30',
    interactive: 'bg-surface-elevated hover:bg-active cursor-pointer transition-colors',
    danger: 'bg-surface-elevated border border-semantic-error/30',
    accent: 'bg-surface-elevated border-brand-signal border-l-4',
  },
  padding: {
    none: '',
    sm: 'p-3',
    md: 'p-4',
    lg: 'p-6',
  },
} as const

export const inputVariants = {
  variant: {
    default: `${colors.surface.muted} border-DEFAULT`,
    error: 'bg-surface-muted border-semantic-error',
  },
  size: {
    sm: 'px-3 py-1.5 text-sm',
    md: 'px-4 py-2 text-sm',
    lg: 'px-4 py-3 text-base',
  },
} as const

export const badgeVariants = {
  color: {
    default: 'bg-status-disconnected/20 text-content-secondary',
    success: colors.semantic.success,
    warning: colors.semantic.warning,
    error: colors.semantic.error,
    info: colors.semantic.info,
    primary: 'bg-brand-signal/20 text-brand-text',
    purple: 'bg-brand-signal/20 text-brand-text',
  },
  size: {
    xs: 'px-1.5 py-0.5 text-[10px]',
    sm: 'px-1.5 py-0.5 text-xs',
    md: 'px-2 py-1 text-sm',
  },
} as const

export const selectVariants = {
  variant: {
    default: `${colors.surface.base} border-DEFAULT`,
  },
  size: {
    sm: 'px-3 py-1.5 text-sm',
    md: 'px-4 py-2 text-sm',
  },
} as const

export const alertVariants = {
  variant: {
    default: 'bg-surface-muted border border-DEFAULT text-content',
    info: 'bg-semantic-info/10 border border-semantic-info/30 text-content',
    success: 'bg-semantic-success/10 border border-semantic-success/30 text-content',
    warning: 'bg-semantic-warning/10 border border-semantic-warning/30 text-content',
    error: 'bg-semantic-error/10 border border-semantic-error/30 text-content',
  },
  iconColor: {
    default: 'text-content-secondary',
    info: 'text-semantic-info',
    success: 'text-semantic-success',
    warning: 'text-semantic-warning',
    error: 'text-semantic-error',
  },
} as const

export const dialogVariants = {
  size: {
    sm: 'max-w-sm',
    md: 'max-w-lg',
    lg: 'max-w-xl',
  },
} as const

export const spinnerVariants = {
  size: {
    sm: 'w-4 h-4',
    md: 'w-6 h-6',
    lg: 'w-10 h-10',
  },
} as const

export const tagBadgeVariants = {
  size: {
    sm: 'px-1.5 py-0.5 text-xs',
    md: 'px-2 py-1 text-sm',
  },
} as const

/** Reusable type aliases derived from variant objects */
export type BadgeColor = keyof typeof badgeVariants.color
export type BadgeSize = keyof typeof badgeVariants.size