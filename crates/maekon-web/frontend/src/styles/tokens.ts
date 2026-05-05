/**
 * Design tokens — semantic Tailwind classes backed by CSS custom properties.
 * Color switching between light/dark is handled entirely by CSS vars in index.css.
 * No `dark:` prefix needed in token values.
 */

/* ── Raw hex palette for charts / inline SVG (no dark-mode switching) ── */
export const palette = {
  teal500: '#14b8a6',
  blue500: '#3b82f6',
  violet500: '#8b5cf6',
  amber500: '#f59e0b',
  red500: '#ef4444',
  emerald500: '#10b981',
  indigo500: '#6366f1',
  pink500: '#ec4899',
  green500: '#22c55e',
  orange500: '#f97316',
  lime500: '#84cc16',
  cyan500: '#06b6d4',
  gray500: '#6B7280',
} as const

export const chartPalette = [
  palette.teal500,
  palette.blue500,
  palette.violet500,
  palette.amber500,
  palette.red500,
  palette.emerald500,
  palette.indigo500,
  palette.pink500,
] as const

/* ── Semantic color tokens ── */
export const colors = {
  primary: {
    DEFAULT: 'bg-brand',
    hover: 'hover:bg-brand-hover',
    text: 'text-brand-text',
    signal: 'bg-brand-signal',
    border: 'border-brand-signal',
  },
  surface: {
    base: 'bg-surface-base',
    elevated: 'bg-surface-elevated',
    muted: 'bg-surface-muted',
    border: 'border-DEFAULT',
    borderMuted: 'border-muted',
  },
  text: {
    primary: 'text-content',
    secondary: 'text-content-secondary',
    tertiary: 'text-content-tertiary',
    inverse: 'text-content-inverse',
    pageTitle: 'text-content',
    pageSubtitle: 'text-content-secondary',
  },
  semantic: {
    success: 'bg-semantic-success/20 text-semantic-success',
    warning: 'bg-semantic-warning/20 text-semantic-warning',
    error: 'bg-semantic-error/20 text-semantic-error',
    info: 'bg-semantic-info/20 text-semantic-info',
  },
  status: {
    connected: 'bg-status-connected',
    connecting: 'bg-status-connecting',
    disconnected: 'bg-status-disconnected',
    error: 'bg-status-error',
  },
} as const

export const spacing = {
  scale: [0, 1, 2, 3, 4, 6, 8, 12] as const,
} as const

export const typography = {
  h1: 'text-2xl font-bold',
  h2: 'text-xl font-semibold',
  h3: 'text-lg font-semibold',
  h4: 'text-base font-medium',
  body: 'text-sm',
  label: 'text-sm font-medium',
  caption: 'text-xs',
  small: 'text-xs',
  micro: 'text-[11px]',
  nano: 'text-[10px]',
  mono: 'font-mono text-sm',
  overline: 'text-[11px] font-semibold uppercase tracking-wider',
  stat: {
    hero: 'text-3xl font-bold',
    large: 'text-2xl font-bold',
    normal: 'text-lg font-medium',
  },
  /* Atomic weight/family tokens for composing ad-hoc styles */
  weight: {
    medium: 'font-medium',
    semibold: 'font-semibold',
    bold: 'font-bold',
  },
  family: {
    mono: 'font-mono',
  },
} as const

export const radius = {
  none: 'rounded-none',
  sm: 'rounded',
  md: 'rounded-lg',
  lg: 'rounded-xl',
  full: 'rounded-full',
} as const

export const interaction = {
  interactive: 'transition-colors duration-150 ease-out',
  focusRing:
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-signal focus-visible:border-transparent',
  disabled: 'disabled:opacity-50 disabled:cursor-not-allowed',
} as const

export const motion = {
  colors: 'transition-colors duration-150 ease-out',
  transform: 'transition-transform duration-300 ease-out',
  opacity: 'transition-opacity duration-200 ease-out',
  all: 'transition-all duration-300 ease-out',
  none: 'transition-none',
  duration: {
    fast: 'duration-150',
    normal: 'duration-300',
    slow: 'duration-500',
  },
} as const

export const elevation = {
  dropdown: 'shadow-lg z-dropdown',
  dialog: 'shadow-xl z-dialog',
  overlay: 'shadow-2xl z-overlay',
  tooltip: 'shadow-md z-tooltip',
} as const

export const iconSize = {
  xs: 'w-3 h-3',
  sm: 'w-3.5 h-3.5',
  base: 'w-4 h-4',
  md: 'w-5 h-5',
  lg: 'w-6 h-6',
  hero: 'w-8 h-8',
} as const

export const form = {
  label: 'block text-sm font-medium text-content-strong mb-2',
  labelSm: 'block text-xs text-content-secondary mb-1',
  helper: 'mt-1 text-xs text-content-secondary',
  checkbox: 'w-5 h-5 rounded bg-surface-base border-DEFAULT text-brand-signal focus-visible:ring-brand-signal',
  checkboxInline:
    'w-5 h-5 rounded bg-surface-base border-DEFAULT text-brand-signal focus-visible:ring-brand-signal mr-3',
  radio: 'w-4 h-4 bg-surface-base border-DEFAULT text-brand-signal focus-visible:ring-brand-signal',
  sectionDivider: 'border-DEFAULT',
} as const

export const dataViz = {
  stroke: {
    good: palette.emerald500,
    warning: palette.amber500,
    critical: palette.red500,
  },
} as const

/* ── Chart tokens — inline style objects for Recharts (CSS vars for theme switching) ── */
export const chart = {
  tooltipStyle: {
    backgroundColor: 'rgb(var(--surface-overlay))',
    border: '1px solid rgb(var(--border-muted))',
    borderRadius: '0.5rem',
    color: 'rgb(var(--content))',
  },
  labelStyle: {
    color: 'rgb(var(--content))',
  },
  gridStroke: 'rgb(var(--border-muted))',
  axis: {
    stroke: palette.gray500,
    tick: { fill: palette.gray500, fontSize: 12 } as const,
  },
} as const

export const layout = {
  titleBar: {
    height: 'h-8',
    bg: 'bg-surface-elevated',
    border: 'border-b border-muted',
    text: 'text-xs font-medium text-content-secondary',
    brand: 'text-sm font-bold text-brand-text',
  },
  activityBar: {
    width: 'w-12',
    bg: 'bg-surface-inset',
    border: 'border-r border-muted',
    iconSize: 'w-5 h-5',
    iconDefault: 'text-content-muted',
    iconActive: 'text-brand-text',
    indicator: 'bg-brand-signal',
    tooltip: 'bg-surface-overlay text-content text-xs px-2 py-1 rounded shadow-lg',
  },
  sidePanel: {
    minWidth: 200,
    maxWidth: 400,
    defaultWidth: 260,
    bg: 'bg-surface-base',
    border: 'border-r border-muted',
    headerBg: 'bg-surface-inset',
    headerText: 'text-[11px] font-semibold uppercase tracking-wider text-content-tertiary',
    itemBg: 'hover:bg-hover',
    itemText: 'text-sm text-content-strong',
    itemActive: 'bg-surface-elevated text-content',
    itemTextParent: 'text-sm font-medium text-content-strong',
    itemTextChild: 'text-[13px] text-content-secondary',
    childBorder: 'border-l-2 border-muted ml-4',
    searchTrigger: 'text-xs text-content-muted bg-surface-inset hover:bg-hover border border-muted rounded',
    resizeHandle: 'w-1 cursor-col-resize hover:bg-brand-signal active:bg-brand-signal transition-colors',
  },
  mainContent: {
    bg: 'bg-surface-sunken',
  },
  statusBar: {
    height: 'h-6',
    bg: 'bg-brand-bar',
    text: 'text-content-inverse text-[11px]',
    separator: 'w-px bg-brand-signal/50 mx-1 h-3.5',
  },
  commandPalette: {
    position: 'pt-[15vh]',
    overlay: 'bg-black/50',
    bg: 'bg-surface-overlay',
    border: 'border border-muted',
    shadow: 'shadow-2xl',
    width: 'w-full max-w-xl',
    input: 'text-base bg-transparent text-content placeholder-content-muted',
    itemBg: 'hover:bg-hover',
    itemActive: 'bg-hover',
    itemText: 'text-sm text-content-strong',
    badge: 'text-[10px] px-1.5 py-0.5 rounded bg-surface-elevated text-content-tertiary',
  },
} as const
