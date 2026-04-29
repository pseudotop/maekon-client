import type { Meta, StoryObj } from '@storybook/react'
import InsightCard from './InsightCard'

const meta = {
  title: 'Domain Components/InsightCard',
  component: InsightCard,
  tags: ['autodocs'],
} satisfies Meta<typeof InsightCard>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    insight: {
      narrative:
        'You spent 4.5 hours in deep focus today, primarily in VS Code working on the authentication module. Your longest uninterrupted session was 1h 42m in the morning.',
      highlights: [
        { highlight_type: 'ACHIEVEMENT', text: 'Longest focus streak this week' },
        { highlight_type: 'WARNING', text: 'High context-switching after 3 PM' },
        { highlight_type: 'SUGGESTION', text: 'Consider batching Slack checks to 30-min intervals' },
      ],
    },
  },
}

export const EmptyState: Story = {
  args: {
    insight: null,
  },
}
