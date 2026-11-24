import type { Meta, StoryObj } from '@storybook/react'
import { useState } from 'react'
import { Checkbox } from './Checkbox'
import { Switch } from './Switch'

const meta = {
  title: 'UI Primitives/Switch',
  component: Switch,
  tags: ['autodocs'],
  argTypes: {
    checked: { control: 'boolean' },
    disabled: { control: 'boolean' },
  },
} satisfies Meta<typeof Switch>

export default meta
type Story = StoryObj<typeof meta>

export const Off: Story = {
  args: { label: 'Process monitoring', checked: false },
}

export const On: Story = {
  args: { label: 'Process monitoring', checked: true },
}

export const WithDescription: Story = {
  args: {
    label: 'Auto-update',
    description: 'Download and install updates when they are available.',
    checked: true,
  },
}

export const DisabledOff: Story = {
  args: { label: 'Pause on battery saver', checked: false, disabled: true },
}

export const DisabledOn: Story = {
  args: { label: 'Pause on battery saver', checked: true, disabled: true },
}

export const Bare: Story = {
  args: { 'aria-label': 'Bare switch (no label prop)', checked: true },
}

function InteractiveSwitch() {
  const [checked, setChecked] = useState(false)
  return <Switch label="Click, Space or Enter toggles me" checked={checked} onCheckedChange={setChecked} />
}

export const Interactive: Story = {
  args: { checked: false },
  render: () => <InteractiveSwitch />,
}

/** Every state side by side; switch the Storybook theme to compare light and dark. */
export const States: Story = {
  args: { checked: false },
  render: () => (
    <div className="space-y-4 bg-surface-elevated p-6">
      <Switch label="Off" checked={false} />
      <Switch label="On" checked />
      <Switch label="Disabled, off" checked={false} disabled />
      <Switch label="Disabled, on" checked disabled />
    </div>
  ),
}

/** A switch is an on/off setting; a list of choices and a confirmation stay checkboxes. */
export const SwitchVersusCheckbox: Story = {
  args: { checked: false },
  render: () => (
    <div className="space-y-6 bg-surface-base p-6">
      <Switch label="Enable notifications" description="Takes effect when you save settings." checked />
      <div className="flex gap-4">
        <Checkbox label="Mon" defaultChecked />
        <Checkbox label="Tue" defaultChecked />
        <Checkbox label="Wed" />
      </div>
      <Checkbox label="I understand this folder may be synced to the cloud" />
    </div>
  ),
}
