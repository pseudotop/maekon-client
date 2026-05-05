import type { Meta, StoryObj } from '@storybook/react'
import { FocusModeIndicator } from './FocusModeIndicator'

const meta = {
  title: 'Overlay/FocusModeIndicator',
  component: FocusModeIndicator,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof FocusModeIndicator>

export default meta
type Story = StoryObj<typeof meta>

/** Badge visible when focus mode is active. */
export const Active: Story = {
  args: { active: true },
}

/** Nothing rendered when inactive. */
export const Inactive: Story = {
  args: { active: false },
}
