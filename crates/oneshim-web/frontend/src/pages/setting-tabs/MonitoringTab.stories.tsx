import type { Meta, StoryObj } from '@storybook/react'
import { createMockDesktopPermissionSnapshot } from '../../stories/mock-data'
import { darkThemeGlobals, lightThemeGlobals, reviewStoryParameters } from '../../stories/storybook-helpers'
import MonitoringTab from './MonitoringTab'
import { makeDefaultFormData } from './stories-utils'

const meta = {
  title: 'Settings/MonitoringTab',
  component: MonitoringTab,
  tags: ['autodocs'],
} satisfies Meta<typeof MonitoringTab>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    formData: makeDefaultFormData(),
    onRootChange: () => {},
    onMonitorChange: () => {},
  },
}

export const CaptureDisabled: Story = {
  args: {
    formData: makeDefaultFormData({ capture_enabled: false }),
    onRootChange: () => {},
    onMonitorChange: () => {},
  },
}

export const PrivacyModeOn: Story = {
  args: {
    formData: makeDefaultFormData({
      monitor: { process_monitoring: true, input_activity: false, privacy_mode: true },
    }),
    onRootChange: () => {},
    onMonitorChange: () => {},
  },
}

export const MacPermissionsAttention: Story = {
  args: {
    formData: makeDefaultFormData(),
    permissionStatus: createMockDesktopPermissionSnapshot(),
    permissionStatusLoading: false,
    onRootChange: () => {},
    onMonitorChange: () => {},
  },
  parameters: reviewStoryParameters,
  globals: lightThemeGlobals,
}

export const MacNotificationPrompt: Story = {
  args: {
    formData: makeDefaultFormData(),
    permissionStatus: createMockDesktopPermissionSnapshot({
      notifications: {
        state: 'needs_attention',
        status_reason: 'macos_notifications_not_determined',
      },
    }),
    permissionStatusLoading: false,
    onRootChange: () => {},
    onMonitorChange: () => {},
    onRequestNotificationPermission: () => {},
  },
  parameters: reviewStoryParameters,
  globals: lightThemeGlobals,
}

export const WindowsBaseline: Story = {
  args: {
    formData: makeDefaultFormData(),
    permissionStatus: createMockDesktopPermissionSnapshot({
      platform: 'windows',
      accessibility: { state: 'not_required', status_reason: null },
      screen_capture: { state: 'not_required', status_reason: null },
    }),
    permissionStatusLoading: false,
    onRootChange: () => {},
    onMonitorChange: () => {},
  },
  globals: darkThemeGlobals,
  parameters: reviewStoryParameters,
}

export const PermissionProbeError: Story = {
  args: {
    formData: makeDefaultFormData(),
    permissionStatusError: 'desktop permission probe failed',
    onRootChange: () => {},
    onMonitorChange: () => {},
    onRefreshPermissionStatus: () => {},
  },
  globals: lightThemeGlobals,
  parameters: reviewStoryParameters,
}
