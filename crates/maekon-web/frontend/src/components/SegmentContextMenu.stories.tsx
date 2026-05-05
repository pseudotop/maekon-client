import type { Meta, StoryObj } from '@storybook/react'
import SegmentContextMenu from './SegmentContextMenu'

const noop = () => {}

const mockRegimeOptions = [
  { id: 'regime-deep', label: 'Deep Work' },
  { id: 'regime-comms', label: 'Communication' },
  { id: 'regime-research', label: 'Research' },
  { id: 'regime-admin', label: 'Administrative' },
  { id: 'regime-break', label: 'Break' },
]

const meta = {
  title: 'Domain Components/SegmentContextMenu',
  component: SegmentContextMenu,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <div className="relative" style={{ minHeight: 300, paddingTop: 20 }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof SegmentContextMenu>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    segmentId: 'seg-1',
    currentRegimeId: 'regime-deep',
    regimeOptions: mockRegimeOptions,
    onMarkAsNoise: noop,
    onReassignRegime: noop,
    onClose: noop,
  },
}
