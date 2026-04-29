import type { Meta, StoryObj } from '@storybook/react'
import { ChartSkeleton, ListSkeleton, Skeleton, StatCardsSkeleton } from './Skeleton'

const meta = {
  title: 'UI Primitives/Skeleton',
  component: Skeleton,
  tags: ['autodocs'],
} satisfies Meta<typeof Skeleton>

export default meta
type Story = StoryObj<typeof meta>

export const Base: Story = {
  render: () => (
    <div className="max-w-md space-y-3">
      <Skeleton className="h-4 w-full" />
      <Skeleton className="h-4 w-3/4" />
      <Skeleton className="h-4 w-1/2" />
    </div>
  ),
}

export const StatCards: Story = {
  render: () => <StatCardsSkeleton />,
}

export const StatCardsCustomCount: Story = {
  render: () => <StatCardsSkeleton count={2} />,
}

export const Chart: Story = {
  render: () => <ChartSkeleton />,
}

export const ChartTall: Story = {
  render: () => <ChartSkeleton height="h-96" />,
}

export const List: Story = {
  render: () => <ListSkeleton />,
}

export const ListFewRows: Story = {
  render: () => <ListSkeleton rows={3} />,
}

export const AllVariants: Story = {
  render: () => (
    <div className="space-y-8">
      <div>
        <p className="mb-2 font-medium text-content-secondary text-xs">Base Skeleton</p>
        <div className="max-w-md space-y-2">
          <Skeleton className="h-4 w-full" />
          <Skeleton className="h-4 w-2/3" />
        </div>
      </div>
      <div>
        <p className="mb-2 font-medium text-content-secondary text-xs">StatCards (4 cards)</p>
        <StatCardsSkeleton />
      </div>
      <div>
        <p className="mb-2 font-medium text-content-secondary text-xs">Chart</p>
        <ChartSkeleton />
      </div>
      <div>
        <p className="mb-2 font-medium text-content-secondary text-xs">List (5 rows)</p>
        <ListSkeleton />
      </div>
    </div>
  ),
}
