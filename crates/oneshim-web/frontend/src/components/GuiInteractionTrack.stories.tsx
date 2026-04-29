import type { Meta, StoryObj } from '@storybook/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import type { GuiHeatmapCell } from '../api/contracts'
import GuiInteractionTrack from './GuiInteractionTrack'

const mockCells: GuiHeatmapCell[] = Array.from({ length: 24 }, (_, i) => ({
  hour: `${String(i).padStart(2, '0')}:00`,
  count: i >= 9 && i <= 17 ? Math.floor(20 + Math.random() * 80) : Math.floor(Math.random() * 10),
}))

function createQueryClient(cells: GuiHeatmapCell[]) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
    },
  })
  client.setQueryData(['gui-heatmap', undefined, undefined], cells)
  return client
}

function WithQueryClient({ children, cells }: { children: ReactNode; cells: GuiHeatmapCell[] }) {
  return <QueryClientProvider client={createQueryClient(cells)}>{children}</QueryClientProvider>
}

const meta = {
  title: 'Domain Components/GuiInteractionTrack',
  component: GuiInteractionTrack,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <WithQueryClient cells={mockCells}>
        <Story />
      </WithQueryClient>
    ),
  ],
} satisfies Meta<typeof GuiInteractionTrack>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {}

export const Empty: Story = {
  decorators: [
    (Story) => (
      <WithQueryClient cells={[]}>
        <Story />
      </WithQueryClient>
    ),
  ],
}
