import type { Meta, StoryObj } from '@storybook/react'
import StatusBar from './StatusBar'

const meta = {
  title: 'Shell/StatusBar',
  component: StatusBar,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <div className="flex flex-col" style={{ width: 900 }}>
        <Story />
      </div>
    ),
  ],
  parameters: {
    docs: {
      description: {
        component:
          'Bottom status bar showing connection state, automation toggle, CPU/RAM usage, and app version. ' +
          'In Storybook the SSE connection is unavailable, so it renders in the offline/disconnected state.',
      },
    },
  },
} satisfies Meta<typeof StatusBar>

export default meta
type Story = StoryObj<typeof meta>

/** Default render — SSE is disconnected in Storybook so metrics show "--". */
export const Default: Story = {}

/** Wrapped in a narrow container to verify responsive truncation. */
export const Narrow: Story = {
  decorators: [
    (Story) => (
      <div className="flex flex-col" style={{ width: 400 }}>
        <Story />
      </div>
    ),
  ],
}
