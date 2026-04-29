import type { Meta, StoryObj } from '@storybook/react'
import { AppMemoryRouter } from '../router/future'
import { darkThemeGlobals, lightThemeGlobals, reviewStoryParameters } from '../stories/storybook-helpers'
import Onboarding from './Onboarding'

const meta = {
  title: 'Pages/Onboarding',
  component: Onboarding,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <AppMemoryRouter>
        <Story />
      </AppMemoryRouter>
    ),
  ],
} satisfies Meta<typeof Onboarding>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    onComplete: () => {},
  },
}

export const LightReview: Story = {
  args: {
    onComplete: () => {},
  },
  globals: lightThemeGlobals,
  parameters: reviewStoryParameters,
}

export const DarkReview: Story = {
  args: {
    onComplete: () => {},
  },
  globals: darkThemeGlobals,
  parameters: reviewStoryParameters,
}
