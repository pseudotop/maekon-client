import type { Meta, StoryObj } from '@storybook/react'
import type { SuggestionViewDto } from '../types'
import { SuggestionsPanel } from './SuggestionsPanel'

const meta = {
  title: 'Overlay/SuggestionsPanel',
  component: SuggestionsPanel,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof SuggestionsPanel>

export default meta
type Story = StoryObj<typeof meta>

const sampleSuggestions: SuggestionViewDto[] = [
  {
    id: 'sug-1',
    title: 'Consider using keyboard shortcuts',
    body: 'You have been switching between the editor and terminal frequently.',
    priority: 'medium',
    category: 'productivity',
    source: 'ai-coach',
    confidence_score: 0.85,
    created_at: '2026-03-27T10:30:00Z',
    is_read: false,
    reasoning: null,
  },
  {
    id: 'sug-2',
    title: 'Break reminder',
    body: 'You have been working for 2 hours straight. A short break improves focus.',
    priority: 'high',
    category: 'health',
    source: 'wellness',
    confidence_score: 0.92,
    created_at: '2026-03-27T10:25:00Z',
    is_read: true,
    reasoning: null,
  },
  {
    id: 'sug-3',
    title: 'Dark mode available',
    body: 'It is evening — consider switching to dark mode to reduce eye strain.',
    priority: 'low',
    category: 'comfort',
    source: 'ai-coach',
    confidence_score: 0.67,
    created_at: '2026-03-27T10:20:00Z',
    is_read: false,
    reasoning: null,
  },
]

/** Panel open with multiple suggestions. */
export const Open: Story = {
  args: {
    open: true,
    suggestions: sampleSuggestions,
    onClose: () => {},
    onRefresh: () => {},
  },
}

/** Panel open but no suggestions available. */
export const Empty: Story = {
  args: {
    open: true,
    suggestions: [],
    onClose: () => {},
    onRefresh: () => {},
  },
}

/** Panel closed — offscreen. */
export const Closed: Story = {
  args: {
    open: false,
    suggestions: sampleSuggestions,
    onClose: () => {},
    onRefresh: () => {},
  },
}
