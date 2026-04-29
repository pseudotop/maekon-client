import type { Meta, StoryObj } from '@storybook/react'
import { TAG_COLORS, TagBadge } from './TagBadge'

const noop = () => {}

const meta = {
  title: 'Domain Components/TagBadge',
  component: TagBadge,
  tags: ['autodocs'],
} satisfies Meta<typeof TagBadge>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    name: 'Deep Work',
    color: '#3b82f6',
  },
}

export const AllColors: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      {['Research', 'Meeting', 'Code Review', 'Design', 'Writing'].map((name, i) => (
        <TagBadge key={name} name={name} color={TAG_COLORS[i]} />
      ))}
    </div>
  ),
}

export const Selected: Story = {
  args: {
    name: 'Focus Time',
    color: '#22c55e',
    selected: true,
    onClick: noop,
  },
}

export const WithRemove: Story = {
  args: {
    name: 'Browsing',
    color: '#ef4444',
    onRemove: noop,
  },
}

export const SmallSize: Story = {
  args: {
    name: 'Quick Tag',
    color: '#8b5cf6',
    size: 'sm',
  },
}
