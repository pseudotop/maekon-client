import type { Meta, StoryObj } from '@storybook/react'
import SuggestionBanner from './SuggestionBanner'

const meta = {
  title: 'Domain Components/SuggestionBanner',
  component: SuggestionBanner,
  tags: ['autodocs'],
} satisfies Meta<typeof SuggestionBanner>

export default meta
type Story = StoryObj<typeof meta>

/**
 * SuggestionBanner fetches data internally via fetchLocalSuggestions.
 * In Storybook (no backend), it will render nothing (returns null when
 * loading completes with no pending suggestions).
 */
export const Default: Story = {}
