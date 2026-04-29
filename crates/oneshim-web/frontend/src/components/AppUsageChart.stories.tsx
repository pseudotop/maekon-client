import type { Meta, StoryObj } from '@storybook/react'
import type { AppUsage } from '../api/contracts'
import AppUsageChart from './AppUsageChart'

const mockApps: AppUsage[] = [
  { name: 'VS Code', duration_secs: 7200, event_count: 342, frame_count: 180 },
  { name: 'Chrome', duration_secs: 5400, event_count: 218, frame_count: 130 },
  { name: 'Slack', duration_secs: 2700, event_count: 89, frame_count: 65 },
  { name: 'Terminal', duration_secs: 1800, event_count: 156, frame_count: 42 },
  { name: 'Figma', duration_secs: 1200, event_count: 74, frame_count: 28 },
  { name: 'Notion', duration_secs: 600, event_count: 31, frame_count: 15 },
]

const meta = {
  title: 'Domain Components/AppUsageChart',
  component: AppUsageChart,
  tags: ['autodocs'],
} satisfies Meta<typeof AppUsageChart>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    apps: mockApps,
  },
}

export const EmptyApps: Story = {
  args: {
    apps: [],
  },
}

export const SingleApp: Story = {
  args: {
    apps: [{ name: 'VS Code', duration_secs: 14400, event_count: 720, frame_count: 360 }],
  },
}
