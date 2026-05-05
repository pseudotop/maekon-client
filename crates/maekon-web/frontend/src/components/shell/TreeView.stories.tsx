import type { Meta, StoryObj } from '@storybook/react'
import TreeView, { type TreeNode } from './TreeView'

const flatNodes: TreeNode[] = [
  { id: 'overview', label: 'Overview' },
  { id: 'metrics', label: 'System Metrics' },
  { id: 'processes', label: 'Active Processes' },
  { id: 'focus', label: 'Focus Score' },
  { id: 'heatmap', label: 'Activity Heatmap' },
]

const nestedNodes: TreeNode[] = [
  { id: 'all', label: 'All Frames' },
  {
    id: 'filters',
    label: 'Filters',
    children: [
      { id: 'by-app', label: 'By Application' },
      { id: 'by-tag', label: 'By Tag' },
      { id: 'by-importance', label: 'By Importance' },
    ],
  },
  {
    id: 'views',
    label: 'Views',
    children: [
      { id: 'grid', label: 'Grid View' },
      { id: 'list', label: 'List View' },
    ],
  },
]

const nodesWithCounts: TreeNode[] = [
  { id: 'recent', label: 'Recent Searches', count: 12 },
  { id: 'favorites', label: 'Favorites', count: 5 },
  { id: 'archived', label: 'Archived', count: 38 },
]

const meta = {
  title: 'Shell/TreeView',
  component: TreeView,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <div style={{ width: 240 }} className="bg-surface p-2">
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof TreeView>

export default meta
type Story = StoryObj<typeof meta>

export const Flat: Story = {
  args: {
    nodes: flatNodes,
    selectedId: 'metrics',
    onSelect: () => {},
  },
}

export const Nested: Story = {
  args: {
    nodes: nestedNodes,
    selectedId: 'by-tag',
    onSelect: () => {},
  },
}

export const WithCounts: Story = {
  args: {
    nodes: nodesWithCounts,
    onSelect: () => {},
  },
}
