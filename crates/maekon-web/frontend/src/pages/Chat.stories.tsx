import type { Meta, StoryObj } from '@storybook/react'
import { AppMemoryRouter } from '../router/future'
import { darkThemeGlobals, lightThemeGlobals, reviewStoryParameters } from '../stories/storybook-helpers'
import Chat from './chat'

const meta = {
  title: 'Pages/Chat',
  component: Chat,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <AppMemoryRouter>
        <div style={{ height: '600px' }}>
          <Story />
        </div>
      </AppMemoryRouter>
    ),
  ],
} satisfies Meta<typeof Chat>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {}

export const LightReview: Story = {
  globals: lightThemeGlobals,
  parameters: reviewStoryParameters,
}

export const DarkReview: Story = {
  globals: darkThemeGlobals,
  parameters: reviewStoryParameters,
}
