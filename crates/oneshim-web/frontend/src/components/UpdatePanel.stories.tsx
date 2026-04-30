import type { Meta, StoryObj } from '@storybook/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import type { UpdateStatus } from '../api/contracts'
import UpdatePanel from './UpdatePanel'

function createQueryClient(initialData?: UpdateStatus) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
    },
  })
  if (initialData) {
    client.setQueryData(['update-status'], initialData)
  }
  return client
}

const idleStatus: UpdateStatus = {
  enabled: true,
  auto_install: false,
  phase: 'idle',
  message: null,
  pending: null,
  revision: 1,
  updated_at: new Date().toISOString(),
}

const pendingStatus: UpdateStatus = {
  enabled: true,
  auto_install: false,
  phase: 'PendingApproval',
  message: 'A new version is available.',
  pending: {
    current_version: '0.4.4',
    latest_version: '0.4.5',
    release_url: 'https://github.com/pseudotop/oneshim-client/releases/tag/v0.4.5',
  },
  revision: 2,
  updated_at: new Date().toISOString(),
}

function WithQueryClient({ children, data }: { children: ReactNode; data?: UpdateStatus }) {
  return <QueryClientProvider client={createQueryClient(data)}>{children}</QueryClientProvider>
}

const meta = {
  title: 'Domain Components/UpdatePanel',
  component: UpdatePanel,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <WithQueryClient data={idleStatus}>
        <Story />
      </WithQueryClient>
    ),
  ],
} satisfies Meta<typeof UpdatePanel>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {}

export const Compact: Story = {
  args: {
    compact: true,
  },
}

export const PendingApproval: Story = {
  decorators: [
    (Story) => (
      <WithQueryClient data={pendingStatus}>
        <Story />
      </WithQueryClient>
    ),
  ],
}
