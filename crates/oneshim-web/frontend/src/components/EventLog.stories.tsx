import type { Meta, StoryObj } from '@storybook/react'
import type { TimelineItem } from '../api/contracts'
import EventLog from './EventLog'

const noop = () => {}

const baseTime = new Date('2026-03-27T09:00:00Z')

function offset(minutes: number): string {
  return new Date(baseTime.getTime() + minutes * 60_000).toISOString()
}

const mockItems: TimelineItem[] = [
  {
    type: 'Event',
    id: 'evt-1',
    timestamp: offset(0),
    event_type: 'AppSwitch',
    app_name: 'VS Code',
    window_title: 'main.rs - oneshim-core',
  },
  {
    type: 'Frame',
    id: 1,
    timestamp: offset(5),
    app_name: 'VS Code',
    window_title: 'main.rs - oneshim-core',
    importance: 0.85,
    image_url: '/frames/001.webp',
  },
  {
    type: 'Event',
    id: 'evt-2',
    timestamp: offset(12),
    event_type: 'WindowFocus',
    app_name: 'Chrome',
    window_title: 'Rust Documentation - Google Chrome',
  },
  {
    type: 'IdlePeriod',
    start: offset(20),
    end: offset(35),
    duration_secs: 900,
  },
  {
    type: 'Frame',
    id: 2,
    timestamp: offset(40),
    app_name: 'Slack',
    window_title: '#engineering - Slack',
    importance: 0.42,
    image_url: '/frames/002.webp',
  },
  {
    type: 'Event',
    id: 'evt-3',
    timestamp: offset(55),
    event_type: 'ContextSwitch',
    app_name: 'Terminal',
    window_title: 'zsh - cargo test',
  },
]

const meta = {
  title: 'Domain Components/EventLog',
  component: EventLog,
  tags: ['autodocs'],
} satisfies Meta<typeof EventLog>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    items: mockItems,
    currentTime: new Date(offset(12)),
    onItemClick: noop,
  },
}

export const EmptyLog: Story = {
  args: {
    items: [],
    currentTime: baseTime,
    onItemClick: noop,
  },
}

export const OnlyEvents: Story = {
  args: {
    items: mockItems.filter((item) => item.type === 'Event'),
    currentTime: new Date(offset(0)),
    onItemClick: noop,
  },
}
