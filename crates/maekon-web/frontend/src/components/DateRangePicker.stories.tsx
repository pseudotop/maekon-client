import type { Meta, StoryObj } from '@storybook/react'
import DateRangePicker from './DateRangePicker'

const noop = () => {}

const meta = {
  title: 'Domain Components/DateRangePicker',
  component: DateRangePicker,
  tags: ['autodocs'],
  args: {
    onRangeChange: noop,
  },
} satisfies Meta<typeof DateRangePicker>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {}

export const PresetWeek: Story = {
  args: {
    initialPreset: '7days',
  },
}

export const PresetMonth: Story = {
  args: {
    initialPreset: '30days',
  },
}

export const CustomRange: Story = {
  args: {
    initialPreset: 'custom',
    initialFrom: '2026-03-01',
    initialTo: '2026-03-15',
  },
}

export const AllPresets: Story = {
  render: () => {
    return (
      <div className="space-y-6">
        <div>
          <p className="mb-2 font-medium text-content-secondary text-xs">Today (default)</p>
          <DateRangePicker onRangeChange={noop} />
        </div>
        <div>
          <p className="mb-2 font-medium text-content-secondary text-xs">7 Days</p>
          <DateRangePicker onRangeChange={noop} initialPreset="7days" />
        </div>
        <div>
          <p className="mb-2 font-medium text-content-secondary text-xs">30 Days</p>
          <DateRangePicker onRangeChange={noop} initialPreset="30days" />
        </div>
        <div>
          <p className="mb-2 font-medium text-content-secondary text-xs">Custom Range</p>
          <DateRangePicker
            onRangeChange={noop}
            initialPreset="custom"
            initialFrom="2026-03-01"
            initialTo="2026-03-15"
          />
        </div>
      </div>
    )
  },
}
