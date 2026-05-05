import type { Meta, StoryObj } from '@storybook/react'
import type { SuggestionViewDto } from '../types'
import { SuggestionItem } from './SuggestionItem'

const meta = {
  title: 'Overlay/SuggestionItem',
  component: SuggestionItem,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <div className="w-80 rounded-lg border border-content-inverse/10 bg-surface-sunken/90">
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof SuggestionItem>

export default meta
type Story = StoryObj<typeof meta>

const baseSuggestion: SuggestionViewDto = {
  id: 'sug-1',
  title: 'Consider using keyboard shortcuts',
  body: 'You have been switching between the editor and terminal frequently. Try using Cmd+` to toggle the integrated terminal.',
  priority: 'medium',
  category: 'productivity',
  source: 'ai-coach',
  confidence_score: 0.85,
  created_at: '2026-03-27T10:30:00Z',
  is_read: false,
  reasoning: null,
}

/** Medium priority suggestion with action buttons. */
export const Default: Story = {
  args: {
    item: baseSuggestion,
    onAction: () => {},
  },
}

/** Critical priority — shows error-colored badge. */
export const Critical: Story = {
  args: {
    item: { ...baseSuggestion, id: 'sug-2', priority: 'critical', title: 'High CPU usage detected' },
    onAction: () => {},
  },
}

/** High priority — shows warning-colored badge. */
export const High: Story = {
  args: {
    item: { ...baseSuggestion, id: 'sug-3', priority: 'high', title: 'Break reminder: 2 hours without pause' },
    onAction: () => {},
  },
}

/** Low priority — subdued badge. */
export const Low: Story = {
  args: {
    item: { ...baseSuggestion, id: 'sug-4', priority: 'low', title: 'Tip: try dark mode for evening work' },
    onAction: () => {},
  },
}
