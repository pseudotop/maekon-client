import type { Meta, StoryObj } from '@storybook/react'
import type { QueryClient } from '@tanstack/react-query'
import GuiInteractionTrack from '../../components/GuiInteractionTrack'
import InsightCard from '../../components/InsightCard'
import StatisticsPanel from '../../components/StatisticsPanel'
import TimelineView from '../../components/TimelineView'
import { Card, CardTitle } from '../../components/ui'
import { createMockDailyDigest, createMockGuiHeatmapCells } from '../mock-data'
import {
  darkThemeGlobals,
  lightThemeGlobals,
  ReviewHeader,
  ReviewNote,
  reviewStoryParameters,
  StorySurface,
  withStoryProviders,
} from '../storybook-helpers'

const digest = createMockDailyDigest()
const dayStart = `${digest.date}T00:00:00Z`
const dayEnd = `${digest.date}T23:59:59Z`

function seedDashboardWorkspace(client: QueryClient) {
  client.setQueryData(['gui-heatmap', dayStart, dayEnd], createMockGuiHeatmapCells())
}

function DashboardWorkspaceTemplate() {
  return (
    <StorySurface>
      <ReviewHeader
        eyebrow="Template Review"
        title="Dashboard Workspace"
        description="Composite workspace for reviewing insight, timeline, activity heatmap, and KPI hierarchy together."
      />

      <div className="space-y-6">
        <ReviewNote>
          This template exists to catch the class of bugs where each component looks correct in isolation but the
          assembled page loses heading hierarchy, spacing rhythm, or subtle contrast.
        </ReviewNote>

        <InsightCard insight={digest.insight} />

        <div className="grid gap-6 xl:grid-cols-[1.7fr_1fr]">
          <div className="space-y-6">
            <TimelineView timeline={digest.timeline} overrides={[]} />
            <GuiInteractionTrack start={dayStart} end={dayEnd} />
          </div>

          <Card variant="default" padding="lg">
            <CardTitle className="mb-4">Review intent</CardTitle>
            <ul className="space-y-2 text-content-secondary text-sm">
              <li>Title and subtitle surfaces must stay distinct from card chrome.</li>
              <li>Insight, timeline, and charts should read as one workspace, not unrelated cards.</li>
              <li>Muted copy must remain legible when the overall canvas is light.</li>
            </ul>
          </Card>
        </div>

        <StatisticsPanel statistics={digest.statistics} />
      </div>
    </StorySurface>
  )
}

const meta = {
  title: 'Templates/DashboardWorkspace',
  component: DashboardWorkspaceTemplate,
  tags: ['autodocs'],
  parameters: reviewStoryParameters,
  decorators: [withStoryProviders({ seedQuery: seedDashboardWorkspace })],
} satisfies Meta<typeof DashboardWorkspaceTemplate>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {}

export const LightReview: Story = {
  globals: lightThemeGlobals,
}

export const DarkReview: Story = {
  globals: darkThemeGlobals,
}
