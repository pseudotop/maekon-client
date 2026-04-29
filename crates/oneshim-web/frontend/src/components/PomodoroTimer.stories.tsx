import type { Meta, StoryObj } from '@storybook/react'
import PomodoroTimer from './PomodoroTimer'

const meta = {
  title: 'Domain Components/PomodoroTimer',
  component: PomodoroTimer,
  tags: ['autodocs'],
} satisfies Meta<typeof PomodoroTimer>

export default meta
type Story = StoryObj<typeof meta>

/**
 * PomodoroTimer fetches data internally via fetchCurrentPomodoro.
 * In Storybook (no backend), it will show the idle "Ready" state
 * with a Start button.
 */
export const Default: Story = {}
