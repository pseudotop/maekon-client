import type { Meta, StoryObj } from '@storybook/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import type { HeatmapCell } from '../api/contracts'
import { ActivityHeatmap } from './ActivityHeatmap'

function generateMockCells(maxValue: number): HeatmapCell[] {
  const cells: HeatmapCell[] = []
  for (let day = 0; day < 7; day++) {
    for (let hour = 0; hour < 24; hour++) {
      const isWorkHour = hour >= 9 && hour <= 17
      const isWeekday = day < 5
      let value = 0
      if (isWeekday && isWorkHour) {
        value = Math.round(maxValue * (0.4 + Math.random() * 0.6))
      } else if (isWeekday) {
        value = Math.round(maxValue * Math.random() * 0.15)
      } else {
        value = Math.round(maxValue * Math.random() * 0.1)
      }
      cells.push({ day, hour, value })
    }
  }
  return cells
}

function createQueryClient(cells: HeatmapCell[], maxValue: number) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Number.POSITIVE_INFINITY } },
  })
  client.setQueryData(['heatmap', 7], {
    from_date: '2026-03-20',
    to_date: '2026-03-27',
    max_value: maxValue,
    cells,
  })
  return client
}

function WithQuery({ children, cells, maxValue }: { children: ReactNode; cells: HeatmapCell[]; maxValue: number }) {
  return <QueryClientProvider client={createQueryClient(cells, maxValue)}>{children}</QueryClientProvider>
}

const meta = {
  title: 'Domain Components/ActivityHeatmap',
  component: ActivityHeatmap,
  tags: ['autodocs'],
} satisfies Meta<typeof ActivityHeatmap>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  decorators: [
    (Story) => (
      <WithQuery cells={generateMockCells(150)} maxValue={150}>
        <Story />
      </WithQuery>
    ),
  ],
  args: {
    days: 7,
    className: 'bg-surface-elevated',
  },
}

export const LowActivity: Story = {
  decorators: [
    (Story) => (
      <WithQuery cells={generateMockCells(20)} maxValue={20}>
        <Story />
      </WithQuery>
    ),
  ],
  args: {
    days: 7,
    className: 'bg-surface-elevated',
  },
}

export const EmptyState: Story = {
  decorators: [
    (Story) => {
      const emptyCells = Array.from({ length: 7 * 24 }, (_, i) => ({
        day: Math.floor(i / 24),
        hour: i % 24,
        value: 0,
      }))
      return (
        <WithQuery cells={emptyCells} maxValue={1}>
          <Story />
        </WithQuery>
      )
    },
  ],
  args: {
    days: 7,
    className: 'bg-surface-elevated',
  },
}
