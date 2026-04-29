import type { Meta, StoryObj } from '@storybook/react'
import type { ReactNode } from 'react'
import { ThemeProvider } from '../contexts/ThemeContext'
import StatisticsPanel from './StatisticsPanel'

function WithTheme({ children }: { children: ReactNode }) {
  return <ThemeProvider>{children}</ThemeProvider>
}

const meta = {
  title: 'Domain Components/StatisticsPanel',
  component: StatisticsPanel,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <WithTheme>
        <Story />
      </WithTheme>
    ),
  ],
} satisfies Meta<typeof StatisticsPanel>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    statistics: {
      deep_work_hours: 4.2,
      communication_hours: 1.5,
      meeting_hours: 2.0,
      context_switches: 14,
      longest_focus_mins: 47,
      longest_focus_content: 'Writing unit tests for auth module',
      regime_distribution: {
        'Deep Work': 42,
        Communication: 18,
        Meetings: 22,
        Browsing: 10,
        Idle: 8,
      },
    },
  },
}

export const WithComparison: Story = {
  args: {
    statistics: {
      deep_work_hours: 5.1,
      communication_hours: 1.2,
      meeting_hours: 1.5,
      context_switches: 9,
      longest_focus_mins: 63,
      longest_focus_content: 'Refactoring database migration scripts',
      regime_distribution: {
        'Deep Work': 51,
        Communication: 12,
        Meetings: 18,
        Browsing: 11,
        Idle: 8,
      },
      comparison: {
        deep_work_delta: 0.9,
        communication_delta: -0.3,
        context_switch_delta: -5,
      },
    },
  },
}
