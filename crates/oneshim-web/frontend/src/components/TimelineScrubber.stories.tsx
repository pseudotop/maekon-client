import type { Meta, StoryObj } from '@storybook/react'
import type { AppSegment, TimelineItem } from '../api/contracts'
import TimelineScrubber from './TimelineScrubber'

const noop = () => {}

const startTime = new Date('2026-03-27T09:00:00Z')
const endTime = new Date('2026-03-27T17:00:00Z')

function timeAt(hours: number, minutes = 0): string {
  return new Date(startTime.getTime() + hours * 3_600_000 + minutes * 60_000).toISOString()
}

const mockSegments: AppSegment[] = [
  { app_name: 'VS Code', start: timeAt(0), end: timeAt(2, 30), color: '#3b82f6' },
  { app_name: 'Chrome', start: timeAt(2, 30), end: timeAt(3, 45), color: '#ef4444' },
  { app_name: 'Slack', start: timeAt(4), end: timeAt(4, 45), color: '#8b5cf6' },
  { app_name: 'VS Code', start: timeAt(5), end: timeAt(6, 30), color: '#3b82f6' },
  { app_name: 'Terminal', start: timeAt(6, 30), end: timeAt(8), color: '#22c55e' },
]

const mockItems: TimelineItem[] = [
  {
    type: 'Event',
    id: 'evt-1',
    timestamp: timeAt(0),
    event_type: 'AppSwitch',
    app_name: 'VS Code',
  },
  {
    type: 'IdlePeriod',
    start: timeAt(3, 45),
    end: timeAt(4),
    duration_secs: 900,
  },
  {
    type: 'Frame',
    id: 1,
    timestamp: timeAt(1, 30),
    app_name: 'VS Code',
    window_title: 'main.rs',
    importance: 0.9,
    image_url: '/frames/001.webp',
  },
]

const meta = {
  title: 'Domain Components/TimelineScrubber',
  component: TimelineScrubber,
  tags: ['autodocs'],
} satisfies Meta<typeof TimelineScrubber>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    startTime,
    endTime,
    currentTime: new Date(timeAt(3)),
    isPlaying: false,
    playbackSpeed: 1,
    segments: mockSegments,
    items: mockItems,
    onTimeChange: noop,
    onPlayPause: noop,
    onSpeedChange: noop,
    onSkipToStart: noop,
    onSkipToEnd: noop,
  },
}

export const Playing: Story = {
  args: {
    startTime,
    endTime,
    currentTime: new Date(timeAt(5, 15)),
    isPlaying: true,
    playbackSpeed: 2,
    segments: mockSegments,
    items: mockItems,
    onTimeChange: noop,
    onPlayPause: noop,
    onSpeedChange: noop,
    onSkipToStart: noop,
    onSkipToEnd: noop,
  },
}
