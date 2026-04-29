import type { Meta, StoryObj } from '@storybook/react'
import { Badge } from './Badge'

const meta = {
  title: 'UI Primitives/Badge',
  component: Badge,
  tags: ['autodocs'],
  argTypes: {
    color: {
      control: 'select',
      options: ['default', 'success', 'warning', 'error', 'info', 'primary', 'purple'],
    },
    size: {
      control: 'select',
      options: ['sm', 'md'],
    },
  },
  args: {
    children: 'Badge',
  },
} satisfies Meta<typeof Badge>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: { color: 'default' },
}

export const Success: Story = {
  args: { color: 'success', children: 'Active' },
}

export const Warning: Story = {
  args: { color: 'warning', children: 'Pending' },
}

export const ErrorState: Story = {
  args: { color: 'error', children: 'Failed' },
}

export const Info: Story = {
  args: { color: 'info', children: 'Info' },
}

export const Primary: Story = {
  args: { color: 'primary', children: 'New' },
}

export const Purple: Story = {
  args: { color: 'purple', children: 'Beta' },
}

export const SmallSize: Story = {
  args: { size: 'sm', children: 'Sm' },
}

export const AllColors: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      <Badge color="default">Default</Badge>
      <Badge color="success">Success</Badge>
      <Badge color="warning">Warning</Badge>
      <Badge color="error">Error</Badge>
      <Badge color="info">Info</Badge>
      <Badge color="primary">Primary</Badge>
      <Badge color="purple">Purple</Badge>
    </div>
  ),
}

export const BothSizes: Story = {
  render: () => (
    <div className="flex flex-wrap items-center gap-2">
      <Badge color="primary" size="sm">
        Small
      </Badge>
      <Badge color="primary" size="md">
        Medium
      </Badge>
    </div>
  ),
}
