import type { Meta, StoryObj } from '@storybook/react'

function StoryReviewWorkflowPage() {
  return (
    <div className="min-h-screen max-w-4xl bg-surface-base p-8">
      <h1 className="mb-6 text-2xl text-content" style={{ fontWeight: 700 }}>
        Story Review Workflow
      </h1>
      <p className="mb-8 text-content-secondary text-sm">
        Maekon Storybook review ladder for atom-to-page composition and theme regression checks.
      </p>

      <Section title="Review Ladder">
        <Table
          rows={[
            ['Atom / Base', 'UI Primitives', 'Single-purpose controls and typography surfaces'],
            ['Molecule', 'Domain Components', 'Small feature groups such as charts, cards, and banners'],
            ['Organism', 'Shell / Settings tabs', 'Composite panels with interaction density'],
            ['Template', 'Templates/*', 'Cross-component workspace compositions used for visual review'],
            ['Page', 'Pages/*', 'Route-level surfaces with real page title and layout context'],
          ]}
        />
      </Section>

      <Section title="Required Review Stories">
        <ul className="list-disc space-y-2 pl-6 text-content-secondary text-sm">
          <li>
            All route-level pages that use the shared page-title token must export `LightReview` and `DarkReview`.
          </li>
          <li>Template stories must exist for shell chrome, dashboard workspace, and form-dense settings surfaces.</li>
          <li>Storybook preview defaults to light theme so inverse-text regressions fail early during review.</li>
        </ul>
      </Section>

      <Section title="Review Sequence">
        <ol className="list-decimal space-y-2 pl-6 text-content-secondary text-sm">
          <li>Review atoms and molecules for token correctness and obvious contrast failures.</li>
          <li>Review organisms for spacing rhythm, grouping, and section-level hierarchy.</li>
          <li>
            Review templates to catch composition bugs that only appear when cards and rails are assembled together.
          </li>
          <li>Review pages in both themes with realistic data or seeded empty states before merging.</li>
        </ol>
      </Section>

      <Section title="Why This Exists">
        <p className="text-content-secondary text-sm">
          The `pageTitle` regression happened because tokens looked reasonable in isolation while the final light-theme
          page composition was never treated as a first-class review artifact. This workflow makes that gap explicit.
        </p>
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
  title: 'Docs/Story Review Workflow',
  component: StoryReviewWorkflowPage,
  tags: ['autodocs'],
  parameters: { layout: 'fullscreen' },
} satisfies Meta<typeof StoryReviewWorkflowPage>

export default meta
type Story = StoryObj<typeof meta>

export const Overview: Story = {}
