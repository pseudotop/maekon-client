import type { Meta, StoryObj } from '@storybook/react'
import DataStorageTab from './DataStorageTab'
import { makeDefaultFormData } from './stories-utils'

const meta = {
  title: 'Settings/DataStorageTab',
  component: DataStorageTab,
  tags: ['autodocs'],
} satisfies Meta<typeof DataStorageTab>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    formData: makeDefaultFormData(),
    storageStats: {
      total_size_bytes: 268435456,
      db_size_bytes: 134217728,
      frames_size_bytes: 134217728,
      frame_count: 12500,
      event_count: 45000,
      metric_count: 180000,
      oldest_data_date: '2026-02-25T00:00:00Z',
      newest_data_date: '2026-03-27T12:00:00Z',
    },
    storageLoading: false,
    exportFormat: 'json',
    exportLoading: null,
    onExportFormatChange: () => {},
    onExport: () => {},
    onRootChange: () => {},
    onTelemetryChange: () => {},
  },
}

export const Loading: Story = {
  args: {
    formData: makeDefaultFormData(),
    storageLoading: true,
    exportFormat: 'json',
    exportLoading: null,
    onExportFormatChange: () => {},
    onExport: () => {},
    onRootChange: () => {},
    onTelemetryChange: () => {},
  },
}

export const ExportingMetrics: Story = {
  args: {
    formData: makeDefaultFormData(),
    storageStats: {
      total_size_bytes: 52428800,
      db_size_bytes: 26214400,
      frames_size_bytes: 26214400,
      frame_count: 3200,
      event_count: 8500,
      metric_count: 42000,
      oldest_data_date: '2026-03-20T00:00:00Z',
      newest_data_date: '2026-03-27T12:00:00Z',
    },
    storageLoading: false,
    exportFormat: 'csv',
    exportLoading: 'metrics',
    onExportFormatChange: () => {},
    onExport: () => {},
    onRootChange: () => {},
    onTelemetryChange: () => {},
  },
}
