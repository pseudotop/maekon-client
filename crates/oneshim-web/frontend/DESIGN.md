# Maekon Design System

Design system guide for `oneshim-web/frontend/`. All visual properties flow from tokens — never hardcode colors, spacing, or typography.

## Principles

1. **Semantic tokens over raw values** — Use `surface-elevated` not `bg-slate-100`. Intent-based naming survives theme changes.
2. **CSS custom properties for theming** — Light/dark switching via `:root` / `.dark` class. No `dark:` prefix anywhere.
3. **Strict lint enforcement** — `scripts/lint-design-tokens.sh` runs in CI. Hardcoded colors, arbitrary spacing, and bare font weights are rejected.
4. **Composition over inheritance** — Components use `cn()` (clsx + tailwind-merge) to merge classes safely. Variants live in `variants.ts`, not inline.

## Token Architecture

All tokens are defined in `src/styles/tokens.ts` and referenced throughout the codebase.

### Color System

Colors use CSS custom properties defined in `src/index.css` as space-separated RGB channels, enabling alpha composition via Tailwind's `withAlpha` utility in `tailwind.config.js`.

There are two access paths — **Tailwind classes** (the full namespace) and **JS imports** from `tokens.ts` (a curated subset). The table below shows the full Tailwind namespace available in CSS:

| Category | Tailwind classes | Purpose |
|----------|-----------------|---------|
| **brand** | `DEFAULT`, `hover`, `text`, `signal`, `bar` | Primary teal identity |
| **surface** | `base`, `elevated`, `muted`, `inset`, `sunken`, `overlay` | Background layers |
| **content** | `DEFAULT`, `secondary`, `tertiary`, `inverse`, `muted`, `strong` | Text hierarchy |
| **semantic** | `success`, `warning`, `error`, `info`, `error-hover`, `warning-hover` | Feedback states |
| **status** | `connected`, `connecting`, `disconnected`, `error` | Connection indicators |
| **border** | `DEFAULT`, `muted`, `strong` | Edge/divider strength |
| **interactive** | `hover`, `active` | Hover/pressed feedback |

The `colors` export in `tokens.ts` provides a subset as pre-composed Tailwind class strings:
- `colors.primary` — `DEFAULT`, `hover`, `text`, `signal`, `border`
- `colors.surface` — `base`, `elevated`, `muted`, `border`, `borderMuted`
- `colors.text` — `primary`, `secondary`, `tertiary`, `inverse`, `pageTitle`, `pageSubtitle`
- `colors.semantic` — `success`, `warning`, `error`, `info` (each combines bg + text)
- `colors.status` — `connected`, `connecting`, `disconnected`, `error`

Extended surface classes (`inset`, `sunken`, `overlay`) and content classes (`muted`, `strong`) are available as raw Tailwind utilities (e.g., `bg-surface-inset`, `text-content-strong`) but not in the `colors` JS object — they appear in `layout.*` tokens instead.

A raw hex palette (`palette` in tokens.ts) exists for chart/SVG contexts where CSS vars don't apply.

### Typography

Tokens map to Tailwind utility combinations:

| Token | Classes | Use |
|-------|---------|-----|
| `h1` | `text-2xl font-bold` | Page titles |
| `h2` | `text-xl font-semibold` | Section headers |
| `h3` | `text-lg font-semibold` | Card headers |
| `h4` | `text-base font-medium` | Sub-section headers |
| `body` | `text-sm` | Default text |
| `label` | `text-sm font-medium` | Form labels |
| `caption` | `text-xs` | Secondary descriptions |
| `micro` | `text-[11px]` | Overline, sidebar headers |
| `nano` | `text-[10px]` | Badge micro-text |
| `mono` | `font-mono text-sm` | Code/technical values |
| `overline` | `text-[11px] font-semibold uppercase tracking-wider` | Section labels |
| `small` | `text-xs` | Alias for caption |
| `stat.hero` | `text-3xl font-bold` | Dashboard hero numbers |
| `stat.large` | `text-2xl font-bold` | Card stat numbers |
| `stat.normal` | `text-lg font-medium` | Inline stat values |

Atomic composition tokens: `weight.medium/semibold/bold`, `family.mono` — use these to compose ad-hoc styles when no preset token fits.

### Spacing

Constrained scale enforced by lint: `[0, 1, 2, 3, 4, 6, 8, 12]` (Tailwind units). Off-scale values and arbitrary `[Npx]` are rejected by CI.

### Motion

| Token | Duration | Use |
|-------|----------|-----|
| `motion.colors` | 150ms ease-out | Color transitions (hover, focus) |
| `motion.transform` | 300ms ease-out | Position/size changes |
| `motion.opacity` | 200ms ease-out | Fade in/out |
| `motion.all` | 300ms ease-out | Multi-property transitions |
| `motion.none` | — | Disable transitions |

Duration sub-tokens for composition: `motion.duration.fast` (150ms), `motion.duration.normal` (300ms), `motion.duration.slow` (500ms).

### Elevation

| Token | Shadow | Z-Index | Use |
|-------|--------|---------|-----|
| `elevation.dropdown` | `shadow-lg` | `z-dropdown` (40) | Dropdowns, popovers |
| `elevation.dialog` | `shadow-xl` | `z-dialog` (50) | Modal dialogs |
| `elevation.overlay` | `shadow-2xl` | `z-overlay` (50) | Full-screen overlays |
| `elevation.tooltip` | `shadow-md` | `z-tooltip` (60) | Tooltips |

Additional named z-index tokens in `tailwind.config.js`: `z-toast` (70), `z-detection` (10000), `z-detection-inspector` (10002), `z-detection-header` (10003).

### Border Radius

| Token | Class | Use |
|-------|-------|-----|
| `radius.none` | `rounded-none` | Sharp edges |
| `radius.sm` | `rounded` | Badges, small elements |
| `radius.md` | `rounded-lg` | Cards, inputs, buttons |
| `radius.lg` | `rounded-xl` | Dialogs, panels |
| `radius.full` | `rounded-full` | Avatars, pills |

### Icon Sizes

| Token | Size | Use |
|-------|------|-----|
| `iconSize.xs` | 12px | Inline indicators |
| `iconSize.sm` | 14px | Compact buttons |
| `iconSize.base` | 16px | Default icons |
| `iconSize.md` | 20px | Sidebar nav |
| `iconSize.lg` | 24px | Headers |
| `iconSize.hero` | 32px | Empty states |

### Interaction

| Token | Classes | Use |
|-------|---------|-----|
| `interaction.interactive` | `transition-colors duration-150 ease-out` | Base transition for interactive elements |
| `interaction.focusRing` | `focus-visible:outline-none focus-visible:ring-2 ...` | Keyboard focus indicator |
| `interaction.disabled` | `disabled:opacity-50 disabled:cursor-not-allowed` | Disabled state |

### Form

Pre-composed class strings for form elements:

| Token | Use |
|-------|-----|
| `form.label` | Standard form label (sm font-medium, mb-2) |
| `form.labelSm` | Compact label (xs, mb-1) |
| `form.helper` | Helper text below inputs |
| `form.checkbox` | Checkbox styling (brand-signal focus ring) |
| `form.checkboxInline` | Inline checkbox variant (with right margin) |
| `form.radio` | Radio button styling |
| `form.sectionDivider` | Horizontal rule between form sections |

### Data Visualization

| Token | Hex | Use |
|-------|-----|-----|
| `dataViz.stroke.good` | `#10b981` (emerald) | Positive metrics |
| `dataViz.stroke.warning` | `#f59e0b` (amber) | Warning thresholds |
| `dataViz.stroke.critical` | `#ef4444` (red) | Critical alerts |

## Component Patterns

### Primitive Structure

Every UI primitive follows this pattern:

```tsx
import { forwardRef } from 'react'
import { interaction, radius } from '../../styles/tokens'
import { buttonVariants } from '../../styles/variants'
import { cn } from '../../utils/cn'

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: keyof typeof buttonVariants.variant
  size?: keyof typeof buttonVariants.size
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = 'primary', size = 'md', ...props }, ref) => (
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
      {...props}
    />
  ),
)
Button.displayName = 'Button'
```

**Rules:**
- `forwardRef` on all primitives — enables ref forwarding for composition
- Props extend native HTML attributes — full DOM compatibility
- `cn()` composes all classes — tokens, variants, then caller's `className` last (override)
- `displayName` always set — required for React DevTools / Storybook
- Sensible defaults — `variant='primary'`, `size='md'`

### Variant Files

Variants live in `src/styles/variants.ts`, not inline. Each component gets a variants object:

```ts
export const cardVariants = {
  variant: {
    default: colors.surface.elevated,
    elevated: colors.surface.muted,
    highlight: 'bg-brand-signal/10 border border-brand-signal/30',
    interactive: 'bg-surface-elevated hover:bg-active cursor-pointer transition-colors',
    danger: 'bg-surface-elevated border border-semantic-error/30',
  },
  padding: { none: '', sm: 'p-3', md: 'p-4', lg: 'p-6' },
}
```

### Class Merging

`cn()` (in `src/utils/cn.ts`) uses `clsx` + `tailwind-merge`:
- `clsx`: conditional class composition
- `tailwind-merge`: resolves Tailwind conflicts (last wins)

```ts
cn('p-4', className)  // caller's className overrides p-4 if conflicting
```

### Dark Mode

Theme switching is CSS-only. The `.dark` class on `<html>` swaps CSS custom property values.

**Wrong:**
```tsx
<div className="bg-white dark:bg-slate-900">  // NEVER
```

**Right:**
```tsx
<div className="bg-surface-base">  // Automatically adapts
```

## Layout Tokens

Layout tokens in `tokens.ts` define the IDE-style shell structure:

| Section | Key Properties |
|---------|---------------|
| **titleBar** | h-8, elevated bg, brand text |
| **activityBar** | w-12, inset bg, icon states |
| **sidePanel** | 200-400px resizable, inset header |
| **mainContent** | sunken bg |
| **statusBar** | h-6, brand bg, inverse text |
| **commandPalette** | overlay bg, shadow-2xl |

## Storybook

- **Version**: 10.2.15
- **Addons**: addon-themes (light/dark toggle), addon-a11y (color-contrast, label checks)
- **Story location**: Co-located with components (`*.stories.tsx`)
- **Naming**: `UI Primitives/ComponentName` for primitives, `Domain Components/ComponentName` for domain-specific

### Story Format

```tsx
import type { Meta, StoryObj } from '@storybook/react'

const meta = {
  title: 'UI Primitives/Button',
  component: Button,
  argTypes: { /* controls */ },
} satisfies Meta<typeof Button>

export default meta
type Story = StoryObj<typeof meta>

export const Primary: Story = { args: { variant: 'primary' } }
export const AllVariants: Story = { render: () => <div>...</div> }
```

## Contribution Rules

1. **Always use tokens** — import from `tokens.ts`. Never hardcode `text-slate-500` or `bg-[#fff]`.
2. **Run lint before commit** — `pnpm run lint:tokens` checks all token violations.
3. **Add variants to `variants.ts`** — not inline in components.
4. **Write stories** — every new component needs at minimum a default story + all-variants story.
5. **Use `cn()` for className** — never raw string concatenation.
6. **Extend existing tokens** — if a new value is needed, add it to `tokens.ts` first, then use it.
7. **No `dark:` prefix** — all theming flows through CSS custom properties.
8. **Lint scope** — `lint-design-tokens.sh` excludes `*.test.tsx`, `*.stories.tsx`, and `DevToolbar.tsx`. Production code is always checked.

## File Map

| File | Purpose |
|------|---------|
| `src/styles/tokens.ts` | All design tokens |
| `src/styles/variants.ts` | Component variant objects |
| `src/index.css` | CSS custom properties (light/dark) |
| `tailwind.config.js` | Tailwind theme extensions |
| `src/utils/cn.ts` | Class merging utility |
| `src/components/ui/` | UI primitives |
| `scripts/lint-design-tokens.sh` | CI lint gate |
| `TOKENS.md` | Token visual reference (see companion doc) |
