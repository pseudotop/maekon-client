# Token Reference

Visual reference for all design tokens. Values are CSS custom properties defined in `src/index.css`, consumed via `tailwind.config.js` and `src/styles/tokens.ts`.

For principles, patterns, and contribution rules, see [DESIGN.md](DESIGN.md). For the `colors` JS export mapping (how CSS vars map to importable class strings), see the "Color System" section in DESIGN.md.

---

## Color Palette

### Brand

| Token | Light | Dark | CSS Variable |
|-------|-------|------|-------------|
| `brand` | `rgb(13,148,136)` teal-600 | `rgb(20,184,166)` teal-500 | `--brand` |
| `brand-hover` | `rgb(15,118,110)` teal-700 | `rgb(45,212,191)` teal-400 | `--brand-hover` |
| `brand-text` | `rgb(13,148,136)` teal-600 | `rgb(45,212,191)` teal-400 | `--brand-text` |
| `brand-signal` | `rgb(20,184,166)` teal-500 | `rgb(45,212,191)` teal-400 | `--brand-signal` |
| `brand-bar` | `rgb(13,148,136)` teal-600 | `rgb(15,118,110)` teal-700 | `--brand-bar` |

### Surface

| Token | Light | Dark | CSS Variable |
|-------|-------|------|-------------|
| `surface-base` | `rgb(255,255,255)` white | `rgb(15,23,42)` slate-900 | `--surface-base` |
| `surface-elevated` | `rgb(241,245,249)` slate-100 | `rgb(30,41,59)` slate-800 | `--surface-elevated` |
| `surface-muted` | `rgb(226,232,240)` slate-200 | `rgb(15,23,42)` slate-900 | `--surface-muted` |
| `surface-inset` | `rgb(248,250,252)` slate-50 | `rgb(2,6,23)` slate-950 | `--surface-inset` |
| `surface-sunken` | `rgb(248,250,252)` slate-50 | `rgb(2,6,23)` slate-950 | `--surface-sunken` |
| `surface-overlay` | `rgb(255,255,255)` white | `rgb(30,41,59)` slate-800 | `--surface-overlay` |

### Content

| Token | Light | Dark | CSS Variable |
|-------|-------|------|-------------|
| `content` | `rgb(15,23,42)` slate-900 | `rgb(255,255,255)` white | `--content` |
| `content-secondary` | `rgb(71,85,105)` slate-600 | `rgb(148,163,184)` slate-400 | `--content-secondary` |
| `content-tertiary` | `rgb(100,116,139)` slate-500 | `rgb(100,116,139)` slate-500 | `--content-tertiary` |
| `content-inverse` | `rgb(255,255,255)` white | `rgb(255,255,255)` white | `--content-inverse` |
| `content-muted` | `rgb(148,163,184)` slate-400 | `rgb(100,116,139)` slate-500 | `--content-muted` |
| `content-strong` | `rgb(51,65,85)` slate-700 | `rgb(203,213,225)` slate-300 | `--content-strong` |

### Border

| Token | Light | Dark | CSS Variable |
|-------|-------|------|-------------|
| `border` | `rgb(203,213,225)` slate-300 | `rgb(51,65,85)` slate-700 | `--border` |
| `border-muted` | `rgb(226,232,240)` slate-200 | `rgb(51,65,85)` slate-700 | `--border-muted` |
| `border-strong` | `rgb(148,163,184)` slate-400 | `rgb(71,85,105)` slate-600 | `--border-strong` |

### Semantic

| Token | Light | Dark | CSS Variable |
|-------|-------|------|-------------|
| `semantic-success` | `rgb(22,163,74)` green-600 | `rgb(74,222,128)` green-400 | `--semantic-success` |
| `semantic-warning` | `rgb(202,138,4)` yellow-600 | `rgb(250,204,21)` yellow-400 | `--semantic-warning` |
| `semantic-error` | `rgb(220,38,38)` red-600 | `rgb(248,113,113)` red-400 | `--semantic-error` |
| `semantic-info` | `rgb(37,99,235)` blue-600 | `rgb(96,165,250)` blue-400 | `--semantic-info` |
| `semantic-error-hover` | `rgb(185,28,28)` red-700 | `rgb(248,113,113)` red-400 | `--semantic-error-hover` |
| `semantic-warning-hover` | `rgb(217,119,6)` amber-600 | `rgb(251,191,36)` yellow-400 | `--semantic-warning-hover` |

### Status

| Token | Light | Dark | CSS Variable |
|-------|-------|------|-------------|
| `status-connected` | `rgb(34,197,94)` green-500 | (same) | `--status-connected` |
| `status-connecting` | `rgb(234,179,8)` yellow-500 | (same) | `--status-connecting` |
| `status-disconnected` | `rgb(100,116,139)` slate-500 | (same) | `--status-disconnected` |
| `status-error` | `rgb(239,68,68)` red-500 | (same) | `--status-error` |

### Interactive

| Token | Light | Dark | CSS Variable |
|-------|-------|------|-------------|
| `hover` | `rgb(241,245,249)` slate-100 | `rgb(51,65,85)` slate-700 | `--hover` |
| `active` | `rgb(226,232,240)` slate-200 | `rgb(30,41,59)` slate-800 | `--active` |

### Chart Palette (Raw Hex)

Fixed colors for charts/SVG — no dark-mode switching.

| Name | Hex | Tailwind Equivalent |
|------|-----|---------------------|
| `teal500` | `#14b8a6` | teal-500 |
| `blue500` | `#3b82f6` | blue-500 |
| `violet500` | `#8b5cf6` | violet-500 |
| `amber500` | `#f59e0b` | amber-500 |
| `red500` | `#ef4444` | red-500 |
| `emerald500` | `#10b981` | emerald-500 |
| `indigo500` | `#6366f1` | indigo-500 |
| `pink500` | `#ec4899` | pink-500 |
| `green500` | `#22c55e` | green-500 |
| `orange500` | `#f97316` | orange-500 |
| `lime500` | `#84cc16` | lime-500 |
| `gray500` | `#6B7280` | gray-500 |

`chartPalette` array uses the first 8 in order: teal, blue, violet, amber, red, emerald, indigo, pink.

---

## Typography Scale

| Token | Classes | Preview |
|-------|---------|---------|
| `h1` | `text-2xl font-bold` | **24px bold** |
| `h2` | `text-xl font-semibold` | **20px semibold** |
| `h3` | `text-lg font-semibold` | **18px semibold** |
| `h4` | `text-base font-medium` | 16px medium |
| `body` | `text-sm` | 14px regular |
| `label` | `text-sm font-medium` | 14px medium |
| `caption` | `text-xs` | 12px regular |
| `small` | `text-xs` | 12px regular (caption alias) |
| `micro` | `text-[11px]` | 11px regular |
| `nano` | `text-[10px]` | 10px regular |
| `mono` | `font-mono text-sm` | `14px monospace` |
| `overline` | `text-[11px] font-semibold uppercase tracking-wider` | 11PX SEMIBOLD SPACED |

### Stat Typography

| Token | Classes | Use |
|-------|---------|-----|
| `stat.hero` | `text-3xl font-bold` | Dashboard hero metrics |
| `stat.large` | `text-2xl font-bold` | Card stat numbers |
| `stat.normal` | `text-lg font-medium` | Inline stat values |

### Atomic Tokens

| Token | Class | Use |
|-------|-------|-----|
| `weight.medium` | `font-medium` | Compose with size tokens |
| `weight.semibold` | `font-semibold` | Compose with size tokens |
| `weight.bold` | `font-bold` | Compose with size tokens |
| `family.mono` | `font-mono` | Compose with size tokens |

---

## Spacing Scale

Allowed values enforced by `lint-design-tokens.sh`:

| Tailwind | px | rem |
|----------|-----|-----|
| `0` | 0 | 0 |
| `1` | 4px | 0.25rem |
| `2` | 8px | 0.5rem |
| `3` | 12px | 0.75rem |
| `4` | 16px | 1rem |
| `6` | 24px | 1.5rem |
| `8` | 32px | 2rem |
| `12` | 48px | 3rem |

Off-scale values (5, 7, 9, 10, 11, 13-15) and arbitrary `[Npx]` are rejected.

---

## Motion

| Token | Classes | Duration |
|-------|---------|----------|
| `motion.colors` | `transition-colors duration-150 ease-out` | 150ms |
| `motion.transform` | `transition-transform duration-300 ease-out` | 300ms |
| `motion.opacity` | `transition-opacity duration-200 ease-out` | 200ms |
| `motion.all` | `transition-all duration-300 ease-out` | 300ms |
| `motion.none` | `transition-none` | — |

### Duration Sub-Tokens

| Token | Class | ms |
|-------|-------|----|
| `motion.duration.fast` | `duration-150` | 150ms |
| `motion.duration.normal` | `duration-300` | 300ms |
| `motion.duration.slow` | `duration-500` | 500ms |

---

## Elevation

| Token | Shadow | Z-Index | Use |
|-------|--------|---------|-----|
| `elevation.dropdown` | `shadow-lg` | `z-dropdown` (40) | Dropdowns, popovers |
| `elevation.dialog` | `shadow-xl` | `z-dialog` (50) | Modal dialogs |
| `elevation.overlay` | `shadow-2xl` | `z-overlay` (50) | Full-screen overlays |
| `elevation.tooltip` | `shadow-md` | `z-tooltip` (60) | Tooltips |

### Full Z-Index Scale

Named z-index tokens in `tailwind.config.js`:

| Token | Value | Use |
|-------|-------|-----|
| `z-dropdown` | 40 | Dropdowns, popovers, goal progress |
| `z-panel` | 45 | Floating panels (suggestions) |
| `z-dialog` | 50 | Modal dialogs, command palette |
| `z-overlay` | 50 | Full-screen overlays, lightbox |
| `z-tooltip` | 60 | Tooltips, capture flash |
| `z-toast` | 70 | Toast notifications |
| `z-detection` | 10000 | Detection overlay base |
| `z-detection-inspector` | 10002 | Detection element inspector |
| `z-detection-header` | 10003 | Detection overlay header |

---

## Border Radius

| Token | Class | Approx |
|-------|-------|--------|
| `radius.none` | `rounded-none` | 0 |
| `radius.sm` | `rounded` | 4px |
| `radius.md` | `rounded-lg` | 8px |
| `radius.lg` | `rounded-xl` | 12px |
| `radius.full` | `rounded-full` | 9999px |

---

## Icon Sizes

| Token | Classes | px |
|-------|---------|-----|
| `iconSize.xs` | `w-3 h-3` | 12px |
| `iconSize.sm` | `w-3.5 h-3.5` | 14px |
| `iconSize.base` | `w-4 h-4` | 16px |
| `iconSize.md` | `w-5 h-5` | 20px |
| `iconSize.lg` | `w-6 h-6` | 24px |
| `iconSize.hero` | `w-8 h-8` | 32px |

---

## Interaction

| Token | Classes |
|-------|---------|
| `interaction.interactive` | `transition-colors duration-150 ease-out` |
| `interaction.focusRing` | `focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-signal focus-visible:border-transparent` |
| `interaction.disabled` | `disabled:opacity-50 disabled:cursor-not-allowed` |

---

## Form

| Token | Classes |
|-------|---------|
| `form.label` | `block text-sm font-medium text-content-strong mb-2` |
| `form.labelSm` | `block text-xs text-content-secondary mb-1` |
| `form.helper` | `mt-1 text-xs text-content-secondary` |
| `form.checkbox` | `w-5 h-5 rounded bg-surface-base border-DEFAULT text-brand-signal focus-visible:ring-brand-signal` |
| `form.checkboxInline` | Same as checkbox + `mr-3` |
| `form.radio` | `w-4 h-4 bg-surface-base border-DEFAULT text-brand-signal focus-visible:ring-brand-signal` |
| `form.sectionDivider` | `border-DEFAULT` |

---

## Data Visualization

| Token | Hex | Use |
|-------|-----|-----|
| `dataViz.stroke.good` | `#10b981` (emerald-500) | Positive / healthy |
| `dataViz.stroke.warning` | `#f59e0b` (amber-500) | Warning threshold |
| `dataViz.stroke.critical` | `#ef4444` (red-500) | Critical / alert |

---

## Layout

### Shell CSS Custom Properties

Defined in `index.css`, used by the `.app-shell` grid:

| Property | Value | Use |
|----------|-------|-----|
| `--titlebar-height` | 32px | Title bar row height |
| `--statusbar-height` | 24px | Status bar row height |
| `--activitybar-width` | 48px | Activity bar column width |
| `--sidebar-width` | 260px | Side panel default width |

### Layout Tokens (JS)

The `layout` export in `tokens.ts` provides pre-composed class strings for each shell region. See [DESIGN.md](DESIGN.md#layout-tokens) for the summary table. Key sections: `titleBar`, `activityBar`, `sidePanel`, `mainContent`, `statusBar`, `commandPalette`.

---

## Component Variants

Defined in `src/styles/variants.ts`. Each variant maps to a pre-composed Tailwind class string.

### Button

| Variant | Visual |
|---------|--------|
| `primary` | Brand bg, white text |
| `secondary` | Muted bg, default text |
| `ghost` | Transparent, hover bg |
| `danger` | Red bg, white text |
| `warning` | Yellow bg, white text |

| Size | Padding |
|------|---------|
| `sm` | `px-3 py-1.5 text-sm` |
| `md` | `px-4 py-2 text-sm` |
| `lg` | `px-6 py-3 text-base` |
| `icon` | `p-2` |

### Card

| Variant | Visual |
|---------|--------|
| `default` | Elevated bg |
| `elevated` | Muted bg |
| `highlight` | Brand signal tint + border |
| `interactive` | Elevated bg, hover active |
| `danger` | Elevated bg, red border |

| Padding | Value |
|---------|-------|
| `none` | 0 |
| `sm` | `p-3` |
| `md` | `p-4` |
| `lg` | `p-6` |

### Badge

| Color | Visual |
|-------|--------|
| `default` | Gray bg, secondary text |
| `success` | Green tint |
| `warning` | Yellow tint |
| `error` | Red tint |
| `info` | Blue tint |
| `primary` | Brand tint |
| `purple` | Brand tint (alias) |

| Size | Padding |
|------|---------|
| `sm` | `px-1.5 py-0.5 text-xs` |
| `md` | `px-2 py-1 text-sm` |

### Input

| Variant | Visual |
|---------|--------|
| `default` | Muted bg, default border |
| `error` | Muted bg, red border |

| Size | Padding |
|------|---------|
| `sm` | `px-3 py-1.5 text-sm` |
| `md` | `px-4 py-2 text-sm` |
| `lg` | `px-4 py-3 text-base` |

### Select

| Variant | Visual |
|---------|--------|
| `default` | Base bg, default border |

| Size | Padding |
|------|---------|
| `sm` | `px-3 py-1.5 text-sm` |
| `md` | `px-4 py-2 text-sm` |
