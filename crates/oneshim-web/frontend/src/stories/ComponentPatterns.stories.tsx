import type { Meta, StoryObj } from '@storybook/react'

function ComponentPatternsPage() {
  return (
    <div className="min-h-screen max-w-3xl bg-surface-base p-8">
      <h1 className="mb-6 text-2xl text-content" style={{ fontWeight: 700 }}>
        Component Patterns
      </h1>
      <p className="mb-8 text-content-secondary text-sm">
        Architecture patterns used by all components in this design system.
      </p>

      <Section title="Primitive Pattern">
        <p className="mb-3 text-content-secondary text-sm">Every UI primitive follows this structure:</p>
        <Code>{`import { forwardRef } from 'react'
import { interaction, radius } from '../../styles/tokens'
import { myVariants } from '../../styles/variants'
import { cn } from '../../utils/cn'

export interface MyComponentProps extends React.HTMLAttributes<HTMLDivElement> {
  variant?: keyof typeof myVariants.variant
  size?: keyof typeof myVariants.size
}

export const MyComponent = forwardRef<HTMLDivElement, MyComponentProps>(
  ({ className, variant = 'default', size = 'md', ...props }, ref) => (
    <div
      ref={ref}
      className={cn(
        radius.md,
        interaction.interactive,
        myVariants.variant[variant],
        myVariants.size[size],
        className,   // caller's className always last
      )}
      {...props}
    />
  ),
)
MyComponent.displayName = 'MyComponent'`}</Code>
        <RuleList
          rules={[
            'forwardRef on all primitives — enables ref forwarding',
            'Props extend native HTML attributes — full DOM compatibility',
            "cn() composes all classes — caller's className last (override wins)",
            'displayName always set — required for React DevTools + Storybook',
          ]}
        />
      </Section>

      <Section title="Class Merging (cn)">
        <p className="mb-3 text-content-secondary text-sm">cn() wraps clsx + tailwind-merge:</p>
        <Code>{`cn('p-4', 'p-6')                   // → 'p-6' (tailwind-merge resolves conflict)
cn('p-4', isActive && 'bg-hover')  // → 'p-4 bg-hover' (conditional)
cn('p-4', className)               // → caller's className overrides if conflicting`}</Code>
      </Section>

      <Section title="Variant Pattern">
        <p className="mb-3 text-content-secondary text-sm">Variants live in variants.ts, not inline:</p>
        <Code>{`export const buttonVariants = {
  variant: {
    primary: 'bg-brand hover:bg-brand-hover text-content-inverse ...',
    secondary: 'bg-surface-muted hover:bg-active text-content ...',
    ghost: 'hover:bg-hover text-content-secondary',
  },
  size: {
    sm: 'px-3 py-1.5 text-sm',
    md: 'px-4 py-2 text-sm',
    lg: 'px-6 py-3 text-base',
  },
} as const`}</Code>
        <RuleList
          rules={[
            'Flat objects only — variants.variant[key], never nested',
            'Use as const for strict TypeScript inference',
            'Import colors from tokens.ts, not hardcoded classes',
          ]}
        />
      </Section>

      <Section title="Dark Mode">
        <Code>{`// WRONG — never use dark: prefix
<div className="bg-white dark:bg-slate-900">

// RIGHT — automatically adapts via CSS vars
<div className="bg-surface-base">`}</Code>
      </Section>

      <Section title="Architecture Rules">
        <table className="w-full text-sm">
          <tbody>
            {[
              ['No React.createContext', 'Use hooks + callback props instead'],
              ['No React.createPortal', 'Use fixed positioning + z-index'],
              ['No new npm dependencies', 'All UI built with native browser APIs'],
              ['No dark: prefix', 'Theming via CSS custom properties'],
              ['Token-only styling', 'lint-design-tokens.sh blocks hardcoded values in CI'],
            ].map(([rule, reason]) => (
              <tr key={rule} className="border-DEFAULT border-b">
                <td className="px-3 py-2 text-brand-text" style={{ fontWeight: 500 }}>
                  {rule}
                </td>
                <td className="px-3 py-2 text-content-secondary">{reason}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </Section>
    </div>
  )
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mb-8">
      <h2 className="mb-3 border-DEFAULT border-b pb-2 text-content text-lg" style={{ fontWeight: 600 }}>
        {title}
      </h2>
      {children}
    </section>
  )
}

function Code({ children }: { children: string }) {
  return (
    <pre className="mb-3 overflow-x-auto rounded-lg bg-surface-elevated p-4 text-content-secondary text-xs">
      <code>{children}</code>
    </pre>
  )
}

function RuleList({ rules }: { rules: string[] }) {
  return (
    <ul className="mt-3 list-disc space-y-1 pl-6 text-content-secondary text-sm">
      {rules.map((rule) => (
        <li key={rule}>{rule}</li>
      ))}
    </ul>
  )
}

const meta = {
  title: 'Docs/Component Patterns',
  component: ComponentPatternsPage,
  tags: ['autodocs'],
  parameters: { layout: 'fullscreen' },
} satisfies Meta<typeof ComponentPatternsPage>

export default meta
type Story = StoryObj<typeof meta>

export const Overview: Story = {}
