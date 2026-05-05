import type { Meta, StoryObj } from '@storybook/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import type { Tag } from '../api/contracts'
import { TagInput } from './TagInput'

const mockTags: Tag[] = [
  { id: 1, name: 'work', color: '#14b8a6', created_at: '2026-01-01T00:00:00Z' },
  { id: 2, name: 'meeting', color: '#3b82f6', created_at: '2026-01-01T00:00:00Z' },
  { id: 3, name: 'research', color: '#8b5cf6', created_at: '2026-01-02T00:00:00Z' },
  { id: 4, name: 'break', color: '#f59e0b', created_at: '2026-01-03T00:00:00Z' },
  { id: 5, name: 'review', color: '#ef4444', created_at: '2026-01-04T00:00:00Z' },
]

function createQueryClient() {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
    },
  })
  client.setQueryData(['tags'], mockTags)
  return client
}

function WithQueryClient({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={createQueryClient()}>{children}</QueryClientProvider>
}

const noop = () => {}

const meta = {
  title: 'Domain Components/TagInput',
  component: TagInput,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <WithQueryClient>
        <Story />
      </WithQueryClient>
    ),
  ],
  args: {
    onAddTag: noop,
    onRemoveTag: noop,
  },
} satisfies Meta<typeof TagInput>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    selectedTags: [],
    placeholder: 'Add a tag...',
  },
}

export const WithTags: Story = {
  args: {
    selectedTags: [mockTags[0], mockTags[2]],
  },
}
