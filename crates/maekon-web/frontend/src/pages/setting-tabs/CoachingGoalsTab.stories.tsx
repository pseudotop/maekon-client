import type { Meta, StoryObj } from '@storybook/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { AppSettings } from '../../api/contracts'
import CoachingSettingsTab from './CoachingSettingsTab'

function createStoryQueryClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Number.POSITIVE_INFINITY, refetchOnWindowFocus: false } },
  })
}

const STUB_SETTINGS = {
  coaching: {
    enabled: true,
    tone: 'Gentle' as const,
    quiet_hours: [],
    profiles: {
      FocusGuard: { enabled: true, min_interval_secs: 300 },
      TimeAware: { enabled: true, min_interval_secs: 300 },
      DeepWorkCoach: { enabled: true, min_interval_secs: 300 },
      ContextRestore: { enabled: true, min_interval_secs: 300 },
      GoalTracker: { enabled: true, min_interval_secs: 300 },
    },
    regime_goals: {},
    locale: 'en',
    overlay_mode: 'Subtle',
  },
} as unknown as AppSettings

const meta = {
  title: 'Settings/CoachingSettingsTab',
  component: CoachingSettingsTab,
  tags: ['autodocs'],
  args: {
    formData: STUB_SETTINGS,
    onCoachingChange: () => {},
  },
} satisfies Meta<typeof CoachingSettingsTab>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  decorators: [
    (Story) => {
      const qc = createStoryQueryClient()
      return (
        <QueryClientProvider client={qc}>
          <Story />
        </QueryClientProvider>
      )
    },
  ],
}

export const WithGoals: Story = {
  decorators: [
    (Story) => {
      const qc = createStoryQueryClient()
      qc.setQueryData(
        ['goal-progress'],
        [
          { regime_label: 'Deep Work', target_minutes: 120, actual_minutes: 85, progress_pct: 70.8 },
          { regime_label: 'Communication', target_minutes: 60, actual_minutes: 42, progress_pct: 70.0 },
          { regime_label: 'Learning', target_minutes: 30, actual_minutes: 30, progress_pct: 100.0 },
        ],
      )
      return (
        <QueryClientProvider client={qc}>
          <Story />
        </QueryClientProvider>
      )
    },
  ],
}

export const WithQuietHours: Story = {
  args: {
    formData: {
      ...STUB_SETTINGS,
      coaching: {
        ...STUB_SETTINGS.coaching,
        quiet_hours: [
          { start: '22:00', end: '08:00' },
          { start: '12:00', end: '13:00' },
        ],
      },
    } as unknown as AppSettings,
  },
  decorators: [
    (Story) => {
      const qc = createStoryQueryClient()
      return (
        <QueryClientProvider client={qc}>
          <Story />
        </QueryClientProvider>
      )
    },
  ],
}
