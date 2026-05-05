import type { Meta, StoryObj } from '@storybook/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { AppMemoryRouter } from '../router/future'
import { createMockDailyDigest, createMockGuiHeatmapCells } from '../stories/mock-data'
import { darkThemeGlobals, lightThemeGlobals, reviewStoryParameters } from '../stories/storybook-helpers'
import DashboardDay from './DashboardDay'

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, staleTime: Infinity } },
})

function createStoryQueryClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Number.POSITIVE_INFINITY } },
  })
}

const today = new Date().toISOString().split('T')[0]
const dayStart = `${today}T00:00:00Z`
const dayEnd = `${today}T23:59:59Z`

const meta = {
  title: 'Pages/DashboardDay',
  component: DashboardDay,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <QueryClientProvider client={queryClient}>
        <AppMemoryRouter>
          <Story />
        </AppMemoryRouter>
      </QueryClientProvider>
    ),
  ],
} satisfies Meta<typeof DashboardDay>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {}

export const WithMockData: Story = {
  decorators: [
    (Story) => {
      const qc = createStoryQueryClient()
      qc.setQueryData(['dashboard-day', today], createMockDailyDigest())
      qc.setQueryData(['overrides', dayStart, dayEnd], [])
      qc.setQueryData(['gui-heatmap', dayStart, dayEnd], createMockGuiHeatmapCells())
      return (
        <QueryClientProvider client={qc}>
          <AppMemoryRouter>
            <Story />
          </AppMemoryRouter>
        </QueryClientProvider>
      )
    },
  ],
}

export const EmptyState: Story = {
  decorators: [
    (Story) => {
      const qc = createStoryQueryClient()
      qc.setQueryData(
        ['dashboard-day', today],
        createMockDailyDigest({
          insight: null,
          timeline: [],
          statistics: {
            deep_work_hours: 0,
            communication_hours: 0,
            meeting_hours: 0,
            context_switches: 0,
            longest_focus_mins: 0,
            longest_focus_content: '',
            regime_distribution: {},
          },
        }),
      )
      qc.setQueryData(['overrides', dayStart, dayEnd], [])
      qc.setQueryData(['gui-heatmap', dayStart, dayEnd], [])
      return (
        <QueryClientProvider client={qc}>
          <AppMemoryRouter>
            <Story />
          </AppMemoryRouter>
        </QueryClientProvider>
      )
    },
  ],
}

export const LightReview: Story = {
  decorators: WithMockData.decorators,
  globals: lightThemeGlobals,
  parameters: reviewStoryParameters,
}

export const DarkReview: Story = {
  decorators: WithMockData.decorators,
  globals: darkThemeGlobals,
  parameters: reviewStoryParameters,
}
