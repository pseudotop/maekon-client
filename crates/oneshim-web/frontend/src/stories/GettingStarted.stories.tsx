import type { Meta, StoryObj } from '@storybook/react'

function GettingStartedPage() {
  return (
    <div className="min-h-screen max-w-3xl bg-surface-base p-8">
      <h1 className="mb-6 text-2xl text-content" style={{ fontWeight: 700 }}>
        Maekon Design System
      </h1>
      <p className="mb-8 text-content-secondary text-sm">
        Component library for the Maekon desktop dashboard application.
      </p>

      <Section title="Quick Start">
        <Code>{`# Install dependencies\npnpm install\n\n# Run Storybook\npnpm storybook\n# → http://localhost:6006\n\n# Build static Storybook\npnpm build-storybook`}</Code>
      </Section>

      <Section title="Component Categories">
        <Table
          rows={[
            ['UI Primitives', 'src/components/ui/', 'Base components (Button, Input, Card, Alert, Dialog)'],
            ['Shell', 'src/components/shell/', 'App layout (TitleBar, ActivityBar, SidePanel, StatusBar)'],
            ['Domain Components', 'src/components/', 'Feature-specific shared components'],
            ['Templates', 'src/stories/templates/', 'Composite review surfaces spanning multiple components'],
            ['Pages', 'src/pages/', 'Route-level page components'],
            ['Overlay', 'src/overlay/components/', 'Detection overlay window components'],
          ]}
        />
      </Section>

      <Section title="Adding a New Component">
        <ol className="list-decimal space-y-1 pl-6 text-content-secondary text-sm">
          <li>
            Create <InlineCode>src/components/ui/MyComponent.tsx</InlineCode> with forwardRef + cn() pattern
          </li>
          <li>
            Add variants to <InlineCode>src/styles/variants.ts</InlineCode>
          </li>
          <li>
            Export from <InlineCode>src/components/ui/index.ts</InlineCode>
          </li>
          <li>
            Create co-located <InlineCode>MyComponent.stories.tsx</InlineCode> with tags: ['autodocs']
          </li>
          <li>
            For templates and route pages, add <InlineCode>LightReview</InlineCode> and{' '}
            <InlineCode>DarkReview</InlineCode> stories
          </li>
          <li>
            Run <InlineCode>pnpm lint</InlineCode> and <InlineCode>pnpm build-storybook</InlineCode>
          </li>
        </ol>
      </Section>

      <Section title="Key Files">
        <Table
          rows={[
            ['src/styles/tokens.ts', 'All design tokens (colors, typography, spacing, motion)'],
            ['src/styles/variants.ts', 'Component variant class strings'],
            ['src/index.css', 'CSS custom properties (light/dark theme values)'],
            ['src/utils/cn.ts', 'Class merging utility (clsx + tailwind-merge)'],
            ['tailwind.config.js', 'Tailwind theme extensions'],
            ['scripts/lint-design-tokens.sh', 'CI lint gate — blocks hardcoded values'],
          ]}
        />
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
    <pre className="overflow-x-auto rounded-lg bg-surface-elevated p-4 text-content-secondary text-xs">
      <code>{children}</code>
    </pre>
  )
}

function InlineCode({ children }: { children: string }) {
  return <code className="rounded bg-surface-muted px-1 py-0.5 text-content text-xs">{children}</code>
}

function Table({ rows }: { rows: string[][] }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <tbody>
          {rows.map((row) => (
            <tr key={row[0]} className="border-DEFAULT border-b">
              {row.map((cell) => (
                <td
                  key={cell}
                  className={`px-3 py-2 ${cell === row[0] ? 'text-brand-text' : 'text-content-secondary'}`}
                  style={cell === row[0] ? { fontWeight: 500 } : undefined}
                >
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

const meta = {
  title: 'Docs/Getting Started',
  component: GettingStartedPage,
  tags: ['autodocs'],
  parameters: { layout: 'fullscreen' },
} satisfies Meta<typeof GettingStartedPage>

export default meta
type Story = StoryObj<typeof meta>

export const Overview: Story = {}
