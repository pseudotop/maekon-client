import type { Meta, StoryObj } from '@storybook/react'
import NotificationSettings from './NotificationSettings'

const meta = {
  title: 'Settings/NotificationSettings',
  component: NotificationSettings,
  tags: ['autodocs'],
} satisfies Meta<typeof NotificationSettings>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    notification: {
      enabled: true,
      idle_notification: true,
      idle_notification_mins: 30,
      long_session_notification: true,
      long_session_mins: 60,
      high_usage_notification: false,
      high_usage_threshold: 90,
    },
    onChange: () => {},
  },
}

export const AllEnabled: Story = {
  args: {
    notification: {
      enabled: true,
      idle_notification: true,
      idle_notification_mins: 15,
      long_session_notification: true,
      long_session_mins: 45,
      high_usage_notification: true,
      high_usage_threshold: 85,
    },
    onChange: () => {},
  },
}

export const Disabled: Story = {
  args: {
    notification: {
      enabled: false,
      idle_notification: true,
      idle_notification_mins: 30,
      long_session_notification: true,
      long_session_mins: 60,
      high_usage_notification: false,
      high_usage_threshold: 90,
    },
    onChange: () => {},
  },
}
