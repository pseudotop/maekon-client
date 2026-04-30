import type { Meta, StoryObj } from '@storybook/react'
import GeneralTab from './GeneralTab'
import { makeDefaultFormData } from './stories-utils'

const meta = {
  title: 'Settings/GeneralTab',
  component: GeneralTab,
  tags: ['autodocs'],
} satisfies Meta<typeof GeneralTab>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    formData: makeDefaultFormData(),
    updateActionPending: false,
    onRootChange: () => {},
    onNotificationChange: () => {},
    onScheduleChange: () => {},
    onUpdateChange: () => {},
    onUpdateAction: () => {},
  },
}

export const WithPendingUpdate: Story = {
  args: {
    formData: makeDefaultFormData(),
    updateStatus: {
      enabled: true,
      auto_install: false,
      phase: 'PendingApproval',
      message: 'Update available: v0.4.5',
      pending: {
        current_version: '0.4.4',
        latest_version: '0.4.5',
        release_url: 'https://github.com/pseudotop/oneshim-client/releases/tag/v0.4.5',
        release_name: 'v0.4.5',
        published_at: '2026-03-27T00:00:00Z',
        download_url: 'https://example.com/download',
      },
      revision: 1,
      updated_at: '2026-03-27T00:00:00Z',
    },
    updateActionPending: false,
    onRootChange: () => {},
    onNotificationChange: () => {},
    onScheduleChange: () => {},
    onUpdateChange: () => {},
    onUpdateAction: () => {},
  },
}

export const UpdateActionPending: Story = {
  args: {
    formData: makeDefaultFormData(),
    updateActionPending: true,
    onRootChange: () => {},
    onNotificationChange: () => {},
    onScheduleChange: () => {},
    onUpdateChange: () => {},
    onUpdateAction: () => {},
  },
}
