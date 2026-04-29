import type { Meta, StoryObj } from '@storybook/react'
import { Activity, Settings, Shield } from 'lucide-react'
import { useState } from 'react'
import { type Tab, Tabs } from './Tabs'

const storyTabs: Tab[] = [
  {
    id: 'overview',
    label: 'Overview',
    icon: <Activity className="h-4 w-4" aria-hidden="true" />,
  },
  {
    id: 'automation',
    label: 'Automation',
    icon: <Shield className="h-4 w-4" aria-hidden="true" />,
  },
  {
    id: 'settings',
    label: 'Settings',
    icon: <Settings className="h-4 w-4" aria-hidden="true" />,
  },
]

const tabsWithDisabledItem: Tab[] = [
  storyTabs[0],
  {
    id: 'privacy',
    label: 'Privacy',
    icon: <Shield className="h-4 w-4" aria-hidden="true" />,
    disabled: true,
  },
  storyTabs[2],
]

function TabsDemo({ tabs }: { tabs: Tab[] }) {
  const [activeTab, setActiveTab] = useState(tabs[0]?.id ?? '')

  return (
    <div className="space-y-4 bg-surface-base p-6">
      <Tabs tabs={tabs} activeTab={activeTab} onTabChange={setActiveTab} ariaLabel="Demo section navigation" />
      <div className="rounded-xl border border-muted bg-surface-elevated p-4 text-content-secondary text-sm">
        Active tab: <span className="font-medium text-content">{activeTab}</span>
      </div>
    </div>
  )
}

const meta = {
  title: 'UI Primitives/Tabs',
  component: Tabs,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof Tabs>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  render: () => <TabsDemo tabs={storyTabs} />,
}

export const WithDisabledTab: Story = {
  render: () => <TabsDemo tabs={tabsWithDisabledItem} />,
}

export const ManyTabs: Story = {
  render: () => {
    const manyTabs: Tab[] = [
      { id: 'general', label: 'General' },
      { id: 'appearance', label: 'Appearance' },
      { id: 'privacy', label: 'Privacy' },
      { id: 'notifications', label: 'Notifications' },
      { id: 'keyboard', label: 'Keyboard' },
      { id: 'ai-models', label: 'AI Models' },
      { id: 'automation', label: 'Automation' },
      { id: 'advanced', label: 'Advanced' },
    ]
    return <TabsDemo tabs={manyTabs} />
  },
}

export const SingleTab: Story = {
  render: () => {
    const single: Tab[] = [{ id: 'only', label: 'Only Tab', icon: <Settings className="h-4 w-4" aria-hidden="true" /> }]
    return <TabsDemo tabs={single} />
  },
}

export const AllDisabled: Story = {
  render: () => {
    const disabled: Tab[] = [
      { id: 'a', label: 'Tab A', disabled: true },
      { id: 'b', label: 'Tab B', disabled: true },
      { id: 'c', label: 'Tab C', disabled: true },
    ]
    return <TabsDemo tabs={disabled} />
  },
}
