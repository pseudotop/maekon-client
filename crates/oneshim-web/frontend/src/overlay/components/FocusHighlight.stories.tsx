import type { Meta, StoryObj } from '@storybook/react'
import type { FocusHighlightPayload } from '../types'
import FocusHighlight from './FocusHighlight'

const meta = {
  title: 'Overlay/FocusHighlight',
  component: FocusHighlight,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof FocusHighlight>

export default meta
type Story = StoryObj<typeof meta>

const singleTarget: FocusHighlightPayload = {
  handle_id: 'h-1',
  targets: [
    {
      candidate_id: 'c-1',
      x: 100,
      y: 80,
      width: 200,
      height: 40,
      color: '#3B82F6',
      label: 'Submit Button',
    },
  ],
}

/** Single highlighted target with label. */
export const SingleTarget: Story = {
  args: { highlight: singleTarget },
}

const multipleTargets: FocusHighlightPayload = {
  handle_id: 'h-2',
  targets: [
    {
      candidate_id: 'c-1',
      x: 50,
      y: 60,
      width: 180,
      height: 36,
      color: '#3B82F6',
      label: 'Search Field',
    },
    {
      candidate_id: 'c-2',
      x: 300,
      y: 120,
      width: 120,
      height: 32,
      color: '#22C55E',
      label: null,
    },
    {
      candidate_id: 'c-3',
      x: 150,
      y: 200,
      width: 250,
      height: 50,
      color: '#F97316',
      label: 'Navigation Menu',
    },
  ],
}

/** Multiple highlighted targets, some without labels. */
export const MultipleTargets: Story = {
  args: { highlight: multipleTargets },
}
