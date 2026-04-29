import type { Meta, StoryObj } from '@storybook/react'
import { Divider } from './Divider'

const meta = {
  title: 'UI Primitives/Divider',
  component: Divider,
  tags: ['autodocs'],
  argTypes: {
    orientation: {
      control: 'radio',
      options: ['horizontal', 'vertical'],
    },
  },
} satisfies Meta<typeof Divider>

export default meta
type Story = StoryObj<typeof meta>

export const Horizontal: Story = {
  args: { orientation: 'horizontal' },
  decorators: [
    (Story) => (
      <div className="w-64 space-y-3 p-4">
        <p className="text-content text-sm">Above</p>
        <Story />
        <p className="text-content text-sm">Below</p>
      </div>
    ),
  ],
}

export const Vertical: Story = {
  args: { orientation: 'vertical' },
  decorators: [
    (Story) => (
      <div className="flex h-12 items-center gap-3 p-4">
        <span className="text-content text-sm">Left</span>
        <Story />
        <span className="text-content text-sm">Right</span>
      </div>
    ),
  ],
}

export const InContext: Story = {
  render: () => (
    <div className="w-72 space-y-3 rounded-lg bg-surface-elevated p-4">
      <p className="font-medium text-content text-sm">Section 1</p>
      <Divider />
      <p className="font-medium text-content text-sm">Section 2</p>
      <Divider />
      <p className="font-medium text-content text-sm">Section 3</p>
    </div>
  ),
}
