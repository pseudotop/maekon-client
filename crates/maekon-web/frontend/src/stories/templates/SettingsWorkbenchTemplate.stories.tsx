import type { Meta, StoryObj } from '@storybook/react'
import DataStorageTab from '../../pages/setting-tabs/DataStorageTab'
import MonitoringTab from '../../pages/setting-tabs/MonitoringTab'
import NotificationSettings from '../../pages/setting-tabs/NotificationSettings'
import PrivacySettings from '../../pages/setting-tabs/PrivacySettings'
import { makeDefaultFormData } from '../../pages/setting-tabs/stories-utils'
import { createMockDesktopPermissionSnapshot, createMockStorageStats } from '../mock-data'
import {
  darkThemeGlobals,
  lightThemeGlobals,
  ReviewHeader,
  ReviewNote,
  reviewStoryParameters,
  StorySurface,
} from '../storybook-helpers'

const baseFormData = makeDefaultFormData()
const reviewFormData = {
  ...baseFormData,
  monitor: {
    ...baseFormData.monitor,
    privacy_mode: true,
  },
  privacy: {
    ...baseFormData.privacy,
    excluded_apps: ['1Password', 'Discord'],
    excluded_app_patterns: ['*bank*', '*wallet*'],
    excluded_title_patterns: ['*secret*', '*private*'],
  },
}

function noop() {}

function SettingsWorkbenchTemplate() {
  return (
    <StorySurface>
      <ReviewHeader
        eyebrow="Template Review"
        title="Settings Workbench"
        description="Form-heavy review surface for permission messaging, dense settings cards, and mixed control states."
      />

      <div className="space-y-6">
        <ReviewNote>
          This template is the main guard against low-contrast labels, ambiguous OS-specific copy, and overly flat form
          groupings in the settings area.
        </ReviewNote>

        <div className="grid gap-6 2xl:grid-cols-2">
          <div className="space-y-6">
            <MonitoringTab
              formData={reviewFormData}
              permissionStatus={createMockDesktopPermissionSnapshot()}
              permissionStatusLoading={false}
              onRootChange={noop}
              onMonitorChange={noop}
            />
            <PrivacySettings privacy={reviewFormData.privacy} onChange={noop} />
          </div>

          <div className="space-y-6">
            <NotificationSettings notification={reviewFormData.notification} onChange={noop} />
            <DataStorageTab
              formData={reviewFormData}
              storageStats={createMockStorageStats()}
              storageLoading={false}
              exportFormat="json"
              exportLoading={null}
              onExportFormatChange={noop}
              onExport={noop}
              onRootChange={noop}
              onTelemetryChange={noop}
            />
          </div>
        </div>
      </div>
    </StorySurface>
  )
}

const meta = {
  title: 'Templates/SettingsWorkbench',
  component: SettingsWorkbenchTemplate,
  tags: ['autodocs'],
  parameters: reviewStoryParameters,
} satisfies Meta<typeof SettingsWorkbenchTemplate>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {}

export const LightReview: Story = {
  globals: lightThemeGlobals,
}

export const DarkReview: Story = {
  globals: darkThemeGlobals,
}
