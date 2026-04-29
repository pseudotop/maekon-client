import type { Meta, StoryObj } from '@storybook/react'
import type { ProcessSnapshot } from '../api/contracts'
import ProcessList from './ProcessList'

const mockSnapshot: ProcessSnapshot = {
  timestamp: '2026-03-27T14:30:45Z',
  processes: [
    { pid: 1024, name: 'Chrome', cpu_usage: 28.5, memory_bytes: 1.2 * 1024 ** 3 },
    { pid: 2048, name: 'VS Code', cpu_usage: 12.3, memory_bytes: 512 * 1024 ** 2 },
    { pid: 4096, name: 'Slack', cpu_usage: 5.1, memory_bytes: 384 * 1024 ** 2 },
    { pid: 8192, name: 'Discord', cpu_usage: 3.7, memory_bytes: 256 * 1024 ** 2 },
    { pid: 16384, name: 'Spotify', cpu_usage: 2.1, memory_bytes: 192 * 1024 ** 2 },
    { pid: 32768, name: 'Figma', cpu_usage: 8.9, memory_bytes: 640 * 1024 ** 2 },
    { pid: 3072, name: 'Terminal', cpu_usage: 1.4, memory_bytes: 96 * 1024 ** 2 },
    { pid: 5120, name: 'Docker', cpu_usage: 6.2, memory_bytes: 1.8 * 1024 ** 3 },
  ],
}

const meta = {
  title: 'Domain Components/ProcessList',
  component: ProcessList,
  tags: ['autodocs'],
} satisfies Meta<typeof ProcessList>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    snapshot: mockSnapshot,
  },
}

export const EmptyState: Story = {
  args: {
    snapshot: { timestamp: '2026-03-27T14:30:45Z', processes: [] },
  },
}

export const SingleProcess: Story = {
  args: {
    snapshot: {
      timestamp: '2026-03-27T14:30:45Z',
      processes: [{ pid: 1024, name: 'Chrome', cpu_usage: 45.2, memory_bytes: 2.4 * 1024 ** 3 }],
    },
  },
}

export const HighLoad: Story = {
  args: {
    snapshot: {
      timestamp: '2026-03-27T14:30:45Z',
      processes: Array.from({ length: 12 }, (_, i) => ({
        pid: (i + 1) * 1024,
        name: [
          'Chrome',
          'VS Code',
          'Slack',
          'Docker',
          'Figma',
          'Node',
          'Webpack',
          'Postgres',
          'Redis',
          'Nginx',
          'Python',
          'Java',
        ][i],
        cpu_usage: Math.round((90 - i * 7) * 10) / 10,
        memory_bytes: (2.5 - i * 0.18) * 1024 ** 3,
      })),
    },
  },
}
