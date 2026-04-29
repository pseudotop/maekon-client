import type { Meta, StoryObj } from '@storybook/react'
import PrivacySettings from './PrivacySettings'

const meta = {
  title: 'Settings/PrivacySettings',
  component: PrivacySettings,
  tags: ['autodocs'],
} satisfies Meta<typeof PrivacySettings>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    privacy: {
      auto_exclude_sensitive: true,
      pii_filter_level: 'Standard',
      excluded_apps: [],
      excluded_app_patterns: [],
      excluded_title_patterns: [],
    },
    onChange: () => {},
  },
}

export const StrictPrivacy: Story = {
  args: {
    privacy: {
      auto_exclude_sensitive: true,
      pii_filter_level: 'Strict',
      excluded_apps: ['1Password', 'Discord', 'Slack'],
      excluded_app_patterns: ['*bank*', '*wallet*'],
      excluded_title_patterns: ['*password*', '*secret*'],
    },
    onChange: () => {},
  },
}

export const PrivacyOff: Story = {
  args: {
    privacy: {
      auto_exclude_sensitive: false,
      pii_filter_level: 'Off',
      excluded_apps: [],
      excluded_app_patterns: [],
      excluded_title_patterns: [],
    },
    onChange: () => {},
  },
}
