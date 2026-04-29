import type { Meta, StoryObj } from '@storybook/react'
import {
  colors,
  dataViz,
  elevation,
  form,
  iconSize,
  interaction,
  motion,
  palette,
  radius,
  spacing,
  typography,
} from '../styles/tokens'
import { badgeVariants, buttonVariants, cardVariants } from '../styles/variants'

function TokenSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mb-8">
      <h2 className="mb-4 border-DEFAULT border-b pb-2 font-semibold text-content text-lg">{title}</h2>
      {children}
    </section>
  )
}

function Swatch({ label, className }: { label: string; className: string }) {
  return (
    <div className="flex items-center gap-3 py-1">
      <div className={`h-10 w-10 rounded-lg border border-DEFAULT ${className}`} />
      <code className="font-mono text-content-secondary text-xs">{label}</code>
    </div>
  )
}

function TextSample({ label, className }: { label: string; className: string }) {
  return (
    <div className="flex items-center gap-3 py-1">
      <span className={className}>Sample text</span>
      <code className="font-mono text-content-tertiary text-xs">{label}</code>
    </div>
  )
}

function DesignTokensPage() {
  return (
    <div className="min-h-screen max-w-4xl bg-surface-base p-6">
      <h1 className="mb-8 font-bold text-2xl text-content">Design Tokens</h1>

      <TokenSection title="CSS Custom Properties">
        <p className="mb-4 text-content-secondary text-sm">
          All colors are backed by CSS custom properties defined in{' '}
          <code className="rounded bg-surface-muted px-1 py-0.5 text-xs">index.css</code>. Light/dark switching is
          handled entirely by the CSS vars — no{' '}
          <code className="rounded bg-surface-muted px-1 py-0.5 text-xs">dark:</code> prefix needed.
        </p>
        <div className="grid grid-cols-3 gap-3">
          {Object.entries(palette).map(([key, hex]) => (
            <div key={key} className="flex items-center gap-2 py-1">
              <div className="h-8 w-8 rounded-md border border-DEFAULT" style={{ backgroundColor: hex }} />
              <div>
                <code className="block font-mono text-content text-xs">{key}</code>
                <code className="font-mono text-[10px] text-content-tertiary">{hex}</code>
              </div>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Primary Colors">
        <div className="grid grid-cols-2 gap-2">
          <Swatch label="primary.DEFAULT" className={colors.primary.DEFAULT} />
          <Swatch label="primary.hover" className={colors.primary.hover} />
          <TextSample label="primary.text" className={colors.primary.text} />
          <Swatch label="primary.signal" className={colors.primary.signal} />
        </div>
      </TokenSection>

      <TokenSection title="Surface Colors">
        <div className="grid grid-cols-2 gap-2">
          <Swatch label="surface.base" className={colors.surface.base} />
          <Swatch label="surface.elevated" className={colors.surface.elevated} />
          <Swatch label="surface.muted" className={colors.surface.muted} />
          <div className="flex items-center gap-3 py-1">
            <div className={`h-10 w-10 rounded-lg ${colors.surface.border} border-2`} />
            <code className="font-mono text-content-secondary text-xs">surface.border</code>
          </div>
        </div>
      </TokenSection>

      <TokenSection title="Text Colors">
        <div className="space-y-1">
          <TextSample label="text.primary" className={colors.text.primary} />
          <TextSample label="text.secondary" className={colors.text.secondary} />
          <TextSample label="text.tertiary" className={colors.text.tertiary} />
          <div className="flex items-center gap-3 py-1">
            <span className={`${colors.text.inverse} rounded bg-content px-2 py-0.5`}>Inverse</span>
            <code className="font-mono text-content-tertiary text-xs">text.inverse</code>
          </div>
        </div>
      </TokenSection>

      <TokenSection title="Semantic Colors">
        <div className="grid grid-cols-2 gap-2">
          {Object.entries(colors.semantic).map(([key, value]) => (
            <div key={key} className="flex items-center gap-3 py-1">
              <span className={`rounded-full px-3 py-1 font-medium text-sm ${value}`}>{key}</span>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Brand Colors">
        <div className="grid grid-cols-2 gap-2">
          <Swatch label="primary.DEFAULT" className={colors.primary.DEFAULT} />
          <TextSample label="primary.text" className={colors.primary.text} />
          <Swatch label="primary.signal" className={colors.primary.signal} />
        </div>
      </TokenSection>

      <TokenSection title="Status Indicators">
        <div className="flex gap-4">
          {Object.entries(colors.status).map(([key, value]) => (
            <div key={key} className="flex items-center gap-2">
              <div className={`h-3 w-3 rounded-full ${value}`} />
              <span className="text-content-secondary text-xs">{key}</span>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Typography">
        <div className="space-y-2">
          {Object.entries(typography).map(([key, value]) => {
            if (typeof value === 'string') {
              return (
                <div key={key} className="flex items-baseline gap-4">
                  <span className={`${value} text-content`}>{key}</span>
                  <code className="font-mono text-content-tertiary text-xs">{value}</code>
                </div>
              )
            }
            return (
              <div key={key} className="ml-4 space-y-1">
                {Object.entries(value).map(([subKey, subValue]) => (
                  <div key={subKey} className="flex items-baseline gap-4">
                    <span className={`${subValue} text-content`}>
                      {key}.{subKey}
                    </span>
                    <code className="font-mono text-content-tertiary text-xs">{subValue}</code>
                  </div>
                ))}
              </div>
            )
          })}
        </div>
      </TokenSection>

      <TokenSection title="Spacing">
        <div className="space-y-2">
          {Object.entries(spacing).map(([key, value]) => (
            <div key={key} className="flex items-center gap-4">
              <div className={`border border-brand/50 bg-brand/20 ${value}`}>
                <div className="h-4 w-4 rounded bg-brand" />
              </div>
              <code className="font-mono text-content-tertiary text-xs">
                {key}: {value || '(none)'}
              </code>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Border Radius">
        <div className="flex flex-wrap gap-4">
          {Object.entries(radius).map(([key, value]) => (
            <div key={key} className="flex flex-col items-center gap-1">
              <div className={`h-12 w-12 bg-brand ${value}`} />
              <code className="font-mono text-content-tertiary text-xs">{key}</code>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Data Visualization">
        <div className="flex gap-4">
          {Object.entries(dataViz.stroke).map(([key, value]) => (
            <div key={key} className="flex items-center gap-2">
              <div className="h-3 w-8 rounded" style={{ backgroundColor: value }} />
              <code className="font-mono text-content-tertiary text-xs">
                {key}: {value}
              </code>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Button Variants">
        <div className="space-y-3">
          <div className="flex flex-wrap gap-2">
            {Object.entries(buttonVariants.variant).map(([key, value]) => (
              <button type="button" key={key} className={`rounded-lg px-4 py-2 text-sm ${value}`}>
                {key}
              </button>
            ))}
          </div>
          <p className="font-mono text-content-tertiary text-xs">
            Sizes: {Object.keys(buttonVariants.size).join(', ')}
          </p>
        </div>
      </TokenSection>

      <TokenSection title="Card Variants">
        <div className="grid grid-cols-2 gap-3">
          {Object.entries(cardVariants.variant).map(([key, value]) => (
            <div key={key} className={`rounded-lg p-4 ${value}`}>
              <p className="font-medium text-content text-sm">{key}</p>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Badge Variants">
        <div className="flex flex-wrap gap-2">
          {Object.entries(badgeVariants.color).map(([key, value]) => (
            <span key={key} className={`inline-flex items-center rounded-full px-2 py-1 font-medium text-sm ${value}`}>
              {key}
            </span>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Interaction Tokens">
        <div className="space-y-1">
          {Object.entries(interaction).map(([key, value]) => (
            <div key={key} className="flex items-center gap-4">
              <code className="font-mono text-content-secondary text-xs">{key}</code>
              <code className="font-mono text-content-tertiary text-xs">{value}</code>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Form Tokens">
        <div className="space-y-1">
          {Object.entries(form).map(([key, value]) => (
            <div key={key} className="flex items-center gap-4">
              <code className="font-mono text-content-secondary text-xs">{key}</code>
              <code className="max-w-md truncate font-mono text-content-tertiary text-xs">{value}</code>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Motion Tokens">
        <div className="space-y-3">
          {Object.entries(motion).map(([key, value]) => (
            <div key={key} className="flex items-center gap-4">
              <div
                className={`h-4 w-16 rounded bg-brand ${value}`}
                style={{ transform: 'scaleX(0.3)', transformOrigin: 'left' }}
              />
              <code className="font-mono text-content-secondary text-xs">{key}</code>
              <code className="font-mono text-content-tertiary text-xs">{value}</code>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Elevation">
        <div className="grid grid-cols-2 gap-4">
          {Object.entries(elevation).map(([key, value]) => (
            <div key={key} className={`rounded-lg bg-surface-elevated p-4 ${value}`}>
              <p className="font-medium text-content text-sm">{key}</p>
              <code className="font-mono text-content-tertiary text-xs">{value}</code>
            </div>
          ))}
        </div>
      </TokenSection>

      <TokenSection title="Icon Sizes">
        <div className="flex flex-wrap items-end gap-6">
          {Object.entries(iconSize).map(([key, value]) => (
            <div key={key} className="flex flex-col items-center gap-2">
              <div className={`${value} rounded bg-brand`} />
              <code className="font-mono text-content-tertiary text-xs">{key}</code>
              <code className="font-mono text-[10px] text-content-tertiary">{value}</code>
            </div>
          ))}
        </div>
      </TokenSection>
    </div>
  )
}

const surfaceTokens = ['surface-base', 'surface-elevated', 'surface-muted', 'surface-inset', 'surface-sunken'] as const
const contentTokens = ['content', 'content-secondary', 'content-tertiary', 'content-muted', 'content-strong'] as const
const brandTokens = ['brand', 'brand-hover', 'brand-signal'] as const
const semanticTokens = ['semantic-success', 'semantic-warning', 'semantic-error', 'semantic-info'] as const

function ThemeComparisonPage() {
  return (
    <div className="min-h-screen bg-surface-base p-6">
      <h1 className="mb-8 text-2xl text-content" style={{ fontWeight: 700 }}>
        Light / Dark Theme Comparison
      </h1>

      <div className="grid grid-cols-2 gap-8">
        <div className="light rounded-lg border border-DEFAULT bg-surface-base p-4">
          <h2 className="mb-4 text-content" style={{ fontWeight: 600 }}>
            Light Mode
          </h2>
          <div className="space-y-2">
            {surfaceTokens.map((t) => (
              <div key={t} className="flex items-center gap-3">
                <div className={`h-8 w-8 rounded border border-DEFAULT bg-${t}`} />
                <code className="text-content-secondary text-xs">{t}</code>
              </div>
            ))}
          </div>
          <div className="mt-4 space-y-1">
            {contentTokens.map((t) => (
              <p key={t} className={`text-${t} text-sm`}>
                {t}
              </p>
            ))}
          </div>
          <div className="mt-4 flex gap-2">
            {brandTokens.map((t) => (
              <div key={t} className={`h-8 w-8 rounded bg-${t}`} />
            ))}
          </div>
          <div className="mt-4 flex gap-2">
            {semanticTokens.map((t) => (
              <div key={t} className={`h-8 w-8 rounded bg-${t}`} />
            ))}
          </div>
        </div>

        <div className="dark rounded-lg border border-DEFAULT bg-surface-base p-4">
          <h2 className="mb-4 text-content" style={{ fontWeight: 600 }}>
            Dark Mode
          </h2>
          <div className="space-y-2">
            {surfaceTokens.map((t) => (
              <div key={t} className="flex items-center gap-3">
                <div className={`h-8 w-8 rounded border border-DEFAULT bg-${t}`} />
                <code className="text-content-secondary text-xs">{t}</code>
              </div>
            ))}
          </div>
          <div className="mt-4 space-y-1">
            {contentTokens.map((t) => (
              <p key={t} className={`text-${t} text-sm`}>
                {t}
              </p>
            ))}
          </div>
          <div className="mt-4 flex gap-2">
            {brandTokens.map((t) => (
              <div key={t} className={`h-8 w-8 rounded bg-${t}`} />
            ))}
          </div>
          <div className="mt-4 flex gap-2">
            {semanticTokens.map((t) => (
              <div key={t} className={`h-8 w-8 rounded bg-${t}`} />
            ))}
          </div>
        </div>
      </div>
    </div>
  )
}

const meta = {
  title: 'Design System/Design Tokens',
  component: DesignTokensPage,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof DesignTokensPage>

export default meta
type Story = StoryObj<typeof meta>

export const AllTokens: Story = {}

export const ThemeComparison: Story = {
  render: () => <ThemeComparisonPage />,
}
