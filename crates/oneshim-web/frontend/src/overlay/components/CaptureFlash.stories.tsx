import type { Meta, StoryObj } from '@storybook/react'
import { CaptureFlash } from './CaptureFlash'

const meta = {
  title: 'Overlay/CaptureFlash',
  component: CaptureFlash,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof CaptureFlash>

export default meta
type Story = StoryObj<typeof meta>

/** No flash visible when timestamp is null. */
export const Idle: Story = {
  args: { timestamp: null },
}

/** Flash visible — border appears briefly then fades. */
export const Triggered: Story = {
  args: { timestamp: new Date().toISOString() },
}
