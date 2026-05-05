import type { Meta, StoryObj } from '@storybook/react'
import TimelineView from './TimelineView'

const mockTimeline = [
  {
    segment_id: 'seg-1',
    start_time: '2026-03-27T09:00:00Z',
    end_time: '2026-03-27T10:30:00Z',
    duration_mins: 90,
    regime_label: 'Deep Work',
    regime_color: '#3b82f6',
    regime_id: 'regime-deep',
    dominant_app: 'VS Code',
    content_summary: [
      { content: 'Refactored authentication module', work_type: 'Coding', mins: 55 },
      { content: 'Reviewed PR #204 comments', work_type: 'Review', mins: 20 },
      { content: 'Updated unit tests', work_type: 'Testing', mins: 15 },
    ],
    annotation: { highlight_type: 'ACHIEVEMENT', text: 'Longest focus streak this week' },
  },
  {
    segment_id: 'seg-2',
    start_time: '2026-03-27T10:30:00Z',
    end_time: '2026-03-27T11:15:00Z',
    duration_mins: 45,
    regime_label: 'Communication',
    regime_color: '#8b5cf6',
    regime_id: 'regime-comms',
    dominant_app: 'Slack',
    content_summary: [
      { content: 'Team standup discussion', work_type: 'Meeting', mins: 15 },
      { content: 'Responded to engineering thread', work_type: 'Chat', mins: 30 },
    ],
  },
  {
    segment_id: 'seg-3',
    start_time: '2026-03-27T11:15:00Z',
    end_time: '2026-03-27T12:00:00Z',
    duration_mins: 45,
    regime_label: 'Research',
    regime_color: '#f59e0b',
    regime_id: 'regime-research',
    dominant_app: 'Chrome',
    content_summary: [
      { content: 'Read Rust async patterns article', work_type: 'Reading', mins: 25 },
      { content: 'Explored tokio documentation', work_type: 'Research', mins: 20 },
    ],
    annotation: { highlight_type: 'SUGGESTION', text: 'Consider bookmarking for later reference' },
  },
  {
    segment_id: 'seg-4',
    start_time: '2026-03-27T13:00:00Z',
    end_time: '2026-03-27T14:30:00Z',
    duration_mins: 90,
    regime_label: 'Deep Work',
    regime_color: '#3b82f6',
    regime_id: 'regime-deep',
    dominant_app: 'VS Code',
    content_summary: [
      { content: 'Implemented gRPC context client', work_type: 'Coding', mins: 70 },
      { content: 'Debugged connection timeout', work_type: 'Debugging', mins: 20 },
    ],
  },
  {
    segment_id: 'seg-5',
    start_time: '2026-03-27T14:30:00Z',
    end_time: '2026-03-27T15:00:00Z',
    duration_mins: 30,
    regime_label: 'Administrative',
    regime_color: '#ef4444',
    regime_id: 'regime-admin',
    dominant_app: 'Notion',
    content_summary: [{ content: 'Updated sprint board and task statuses', work_type: 'Admin', mins: 30 }],
    annotation: { highlight_type: 'WARNING', text: 'Frequent context switches detected' },
  },
]

const meta = {
  title: 'Domain Components/TimelineView',
  component: TimelineView,
  tags: ['autodocs'],
} satisfies Meta<typeof TimelineView>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    timeline: mockTimeline,
  },
}

export const EmptyTimeline: Story = {
  args: {
    timeline: [],
  },
}
