import type { Meta, StoryObj } from '@storybook/react'
import { Checkbox } from './Checkbox'

const meta = {
  title: 'UI Primitives/Checkbox',
  component: Checkbox,
  tags: ['autodocs'],
  argTypes: {
    checked: { control: 'boolean' },
    disabled: { control: 'boolean' },
  },
} satisfies Meta<typeof Checkbox>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: { label: 'Enable notifications' },
}

export const WithDescription: Story = {
  args: {
    label: 'Auto-update',
    description: 'Automatically install updates when available.',
  },
}

export const Checked: Story = {
  args: { label: 'I agree to the terms', checked: true, readOnly: true },
}

export const Disabled: Story = {
  args: { label: 'Premium feature', disabled: true },
}

export const Bare: Story = {
  args: {},
  decorators: [
    (Story) => (
      <div className="flex items-center gap-2">
        <Story />
        <span className="text-content text-sm">Bare checkbox (no label prop)</span>
      </div>
    ),
  ],
}
