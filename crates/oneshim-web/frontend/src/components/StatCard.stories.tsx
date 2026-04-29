import type { Meta, StoryObj } from '@storybook/react'
import { Activity, Clock, Cpu, Zap } from 'lucide-react'
import { iconSize } from '../styles/tokens'
import StatCard from './StatCard'

const meta = {
  title: 'Domain Components/StatCard',
  component: StatCard,
  tags: ['autodocs'],
} satisfies Meta<typeof StatCard>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    title: 'Active Time',
    value: '4h 32m',
    icon: <Clock className={iconSize.md} />,
  },
}

export const AllMetrics: Story = {
  render: () => (
    <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
      <StatCard title="Active Time" value="4h 32m" icon={<Clock className={iconSize.md} />} />
      <StatCard title="Events" value="1,247" icon={<Activity className={iconSize.md} />} />
      <StatCard title="CPU Average" value="38.5%" icon={<Cpu className={iconSize.md} />} />
      <StatCard title="Productivity" value="87%" icon={<Zap className={iconSize.md} />} />
    </div>
  ),
}

export const LargeValue: Story = {
  args: {
    title: 'Total Events',
    value: '123,456',
    icon: <Activity className={iconSize.md} />,
  },
}

export const ShortValue: Story = {
  args: {
    title: 'Score',
    value: '92',
    icon: <Zap className={iconSize.md} />,
  },
}
