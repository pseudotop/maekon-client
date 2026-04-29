import type { Meta, StoryObj } from '@storybook/react'
import DetectionHeader from './DetectionHeader'

const meta = {
  title: 'Overlay/DetectionHeader',
  component: DetectionHeader,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof DetectionHeader>

export default meta
type Story = StoryObj<typeof meta>

/** Header showing detected element count with action buttons. */
export const Default: Story = {
  args: {
    elementCount: 42,
    onRefresh: () => {},
    onClose: () => {},
  },
}

/** No elements detected. */
export const Empty: Story = {
  args: {
    elementCount: 0,
    onRefresh: () => {},
    onClose: () => {},
  },
}

/** Many elements detected. */
export const ManyElements: Story = {
  args: {
    elementCount: 256,
    onRefresh: () => {},
    onClose: () => {},
  },
}
