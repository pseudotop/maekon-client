import type { Meta, StoryObj } from '@storybook/react'
import { AppMemoryRouter } from '../router/future'
import FocusWidget from './FocusWidget'

const meta = {
  title: 'Domain Components/FocusWidget',
  component: FocusWidget,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <AppMemoryRouter>
        <Story />
      </AppMemoryRouter>
    ),
  ],
} satisfies Meta<typeof FocusWidget>

export default meta
type Story = StoryObj<typeof meta>

/**
 * FocusWidget fetches data internally via fetchFocusMetrics.
 * In Storybook (no backend), it will display the error state
 * after the fetch fails.
 */
export const Default: Story = {}
