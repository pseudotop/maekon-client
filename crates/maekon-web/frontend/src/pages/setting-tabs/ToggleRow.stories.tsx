import type { Meta, StoryObj } from '@storybook/react'
import ToggleRow from './ToggleRow'

const meta = {
  title: 'Settings/ToggleRow',
  component: ToggleRow,
  tags: ['autodocs'],
  argTypes: {
    checked: { control: 'boolean' },
  },
} satisfies Meta<typeof ToggleRow>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    label: 'Process Monitoring',
    description: 'Track active applications and window titles',
    checked: false,
    onChange: () => {},
  },
}

export const Checked: Story = {
  args: {
    label: 'Process Monitoring',
    description: 'Track active applications and window titles',
    checked: true,
    onChange: () => {},
  },
}

export const LongDescription: Story = {
  args: {
    label: 'Privacy Mode',
    description:
      'When enabled, all monitoring data is processed locally and never sent to external servers. Screenshots are redacted before storage.',
    checked: true,
    onChange: () => {},
  },
}
