import type { Meta, StoryObj } from '@storybook/react'
import { Inbox, Search } from 'lucide-react'
import { EmptyState } from './EmptyState'

const meta = {
  title: 'UI Primitives/EmptyState',
  component: EmptyState,
  tags: ['autodocs'],
} satisfies Meta<typeof EmptyState>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    icon: <Inbox className="h-8 w-8" />,
    title: 'No items found',
    description: 'There are no items to display. Try adjusting your filters or create a new item.',
  },
}

export const WithAction: Story = {
  args: {
    icon: <Search className="h-8 w-8" />,
    title: 'No results',
    description: 'No results match your search query. Try a different keyword.',
    action: {
      label: 'Clear search',
      onClick: () => {},
    },
  },
}
