import type { Meta, StoryObj } from '@storybook/react'
import type { AppSettings } from '../../api/client'
import PrivacyTab from './PrivacyTab'
import { makeDefaultFormData } from './stories-utils'

const meta = {
  title: 'Settings/PrivacyTab',
  component: PrivacyTab,
  tags: ['autodocs'],
} satisfies Meta<typeof PrivacyTab>

export default meta
type Story = StoryObj<typeof meta>

const formData: AppSettings = makeDefaultFormData()

export const Default: Story = {
  args: {
    formData,
    onPrivacyChange: () => {},
  },
}

export const StrictMode: Story = {
  args: {
    formData: {
      ...formData,
      privacy: {
        auto_exclude_sensitive: true,
        pii_filter_level: 'Strict',
        excluded_apps: ['1Password', 'Telegram'],
        excluded_app_patterns: ['*bank*'],
        excluded_title_patterns: ['*password*'],
      },
    },
    onPrivacyChange: () => {},
  },
}
