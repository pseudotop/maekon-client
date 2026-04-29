import type { Meta, StoryObj } from '@storybook/react'
import type { ReactNode } from 'react'
import type { HourlyMetrics } from '../api/contracts'
import { ThemeProvider } from '../contexts/ThemeContext'
import MetricsChart from './MetricsChart'

function WithTheme({ children }: { children: ReactNode }) {
  return <ThemeProvider>{children}</ThemeProvider>
}

function generateMockData(hours: number): HourlyMetrics[] {
  const now = new Date()
  return Array.from({ length: hours }, (_, i) => {
    const date = new Date(now.getTime() - (hours - 1 - i) * 3600000)
    const hour = date.getHours()
    const isWorkHour = hour >= 9 && hour <= 17
    const baseCpu = isWorkHour ? 40 + Math.random() * 30 : 10 + Math.random() * 15
    const baseMem = isWorkHour ? 4 + Math.random() * 2 : 2 + Math.random() * 1.5

    return {
      hour: date.toISOString(),
      cpu_avg: Math.round(baseCpu * 10) / 10,
      cpu_max: Math.round((baseCpu + 15 + Math.random() * 10) * 10) / 10,
      memory_avg: baseMem * 1024 * 1024 * 1024,
      memory_max: (baseMem + 0.8) * 1024 * 1024 * 1024,
      sample_count: 60,
    }
  })
}

const meta = {
  title: 'Domain Components/MetricsChart',
  component: MetricsChart,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <WithTheme>
        <Story />
      </WithTheme>
    ),
  ],
} satisfies Meta<typeof MetricsChart>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    data: generateMockData(24),
  },
}

export const FewDataPoints: Story = {
  args: {
    data: generateMockData(6),
  },
}

export const EmptyData: Story = {
  args: {
    data: [],
  },
}
