import type { Meta, StoryObj } from '@storybook/react'
import ScheduleSettings from './ScheduleSettings'

const meta = {
  title: 'Settings/ScheduleSettings',
  component: ScheduleSettings,
  tags: ['autodocs'],
} satisfies Meta<typeof ScheduleSettings>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    schedule: {
      active_hours_enabled: true,
      active_start_hour: 9,
      active_end_hour: 18,
      active_days: ['Mon', 'Tue', 'Wed', 'Thu', 'Fri'],
      pause_on_screen_lock: true,
      pause_on_battery_saver: false,
    },
    onChange: () => {},
  },
}

export const Disabled: Story = {
  args: {
    schedule: {
      active_hours_enabled: false,
      active_start_hour: 9,
      active_end_hour: 18,
      active_days: ['Mon', 'Tue', 'Wed', 'Thu', 'Fri'],
      pause_on_screen_lock: false,
      pause_on_battery_saver: false,
    },
    onChange: () => {},
  },
}

export const WeekendSchedule: Story = {
  args: {
    schedule: {
      active_hours_enabled: true,
      active_start_hour: 10,
      active_end_hour: 22,
      active_days: ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'],
      pause_on_screen_lock: true,
      pause_on_battery_saver: true,
    },
    onChange: () => {},
  },
}
