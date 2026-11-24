import type { Meta, StoryObj } from '@storybook/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { AppMemoryRouter } from '../router/future'
import { darkThemeGlobals, lightThemeGlobals, reviewStoryParameters } from '../stories/storybook-helpers'
import Onboarding, { ONBOARDING_STEP_STORAGE_KEY } from './Onboarding'

/** Opens a story on one step; the page keeps its step in storage across a restart (#13734). */
function openAtStep(step: number) {
  return () => {
    try {
      if (step > 0) {
        window.localStorage.setItem(ONBOARDING_STEP_STORAGE_KEY, String(step))
      } else {
        window.localStorage.removeItem(ONBOARDING_STEP_STORAGE_KEY)
      }
    } catch {
      // Without storage the story opens on the first step.
    }
  }
}

const meta = {
  title: 'Pages/Onboarding',
  component: Onboarding,
  tags: ['autodocs'],
  // The consent and AI steps read and save through react-query, as they do under main.tsx.
  decorators: [
    (Story) => (
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <AppMemoryRouter>
          <Story />
        </AppMemoryRouter>
      </QueryClientProvider>
    ),
  ],
  beforeEach: openAtStep(0),
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

/** Without the desktop app there is no consent record, so the step asks for the full-text consent first. */
export const AiStep: Story = {
  args: {
    onComplete: () => {},
  },
  beforeEach: openAtStep(3),
  globals: lightThemeGlobals,
  parameters: reviewStoryParameters,
}

export const ReadyStep: Story = {
  args: {
    onComplete: () => {},
  },
  beforeEach: openAtStep(4),
  globals: lightThemeGlobals,
  parameters: reviewStoryParameters,
}
