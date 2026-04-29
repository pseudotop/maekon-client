import type { Meta, StoryObj } from '@storybook/react'
import type { GoalProgressItem } from '../types'
import GoalProgressBar from './GoalProgressBar'

const meta = {
  title: 'Overlay/GoalProgressBar',
  component: GoalProgressBar,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof GoalProgressBar>

export default meta
type Story = StoryObj<typeof meta>

const sampleGoals: GoalProgressItem[] = [
  { regime_label: 'Deep Work', current_minutes: 45, target_minutes: 120, percentage: 37.5, display_color: '#3B82F6' },
  { regime_label: 'Communication', current_minutes: 30, target_minutes: 60, percentage: 50, display_color: '#22C55E' },
  { regime_label: 'Planning', current_minutes: 15, target_minutes: 30, percentage: 50, display_color: '#F59E0B' },
]

/** Multiple goals with varying progress. */
export const Default: Story = {
  args: { goals: sampleGoals },
}

/** All goals completed — bars at 100%. */
export const AllComplete: Story = {
  args: {
    goals: [
      {
        regime_label: 'Deep Work',
        current_minutes: 120,
        target_minutes: 120,
        percentage: 100,
        display_color: '#3B82F6',
      },
      { regime_label: 'Exercise', current_minutes: 30, target_minutes: 30, percentage: 100, display_color: '#22C55E' },
    ],
  },
}

/** Single goal just started. */
export const SingleGoal: Story = {
  args: {
    goals: [
      { regime_label: 'Coding', current_minutes: 5, target_minutes: 180, percentage: 2.8, display_color: '#8B5CF6' },
    ],
  },
}
