import type { Meta, StoryObj } from '@storybook/react'
import { Select } from './Select'

const meta = {
  title: 'UI Primitives/Select',
  component: Select,
  tags: ['autodocs'],
  argTypes: {
    selectSize: {
      control: 'select',
      options: ['sm', 'md'],
    },
    disabled: { control: 'boolean' },
  },
} satisfies Meta<typeof Select>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  render: (args) => (
    <Select {...args}>
      <option value="">Select an option</option>
      <option value="1">Option 1</option>
      <option value="2">Option 2</option>
      <option value="3">Option 3</option>
    </Select>
  ),
  args: { selectSize: 'md' },
}

export const Small: Story = {
  render: (args) => (
    <Select {...args}>
      <option value="">Select...</option>
      <option value="a">Alpha</option>
      <option value="b">Bravo</option>
    </Select>
  ),
  args: { selectSize: 'sm' },
}

export const Disabled: Story = {
  render: (args) => (
    <Select {...args}>
      <option value="">Disabled select</option>
      <option value="1">Option 1</option>
    </Select>
  ),
  args: { disabled: true },
}

export const WithLabel: Story = {
  render: () => (
    <div className="max-w-xs space-y-4">
      <div>
        <label htmlFor="country" className="mb-2 block font-medium text-content-strong text-sm">
          Country
        </label>
        <Select id="country">
          <option value="">Choose a country</option>
          <option value="kr">South Korea</option>
          <option value="us">United States</option>
          <option value="jp">Japan</option>
        </Select>
      </div>
      <div>
        <label htmlFor="role" className="mb-2 block font-medium text-content-strong text-sm">
          Role
        </label>
        <Select id="role" selectSize="sm">
          <option value="">Select role</option>
          <option value="admin">Admin</option>
          <option value="user">User</option>
        </Select>
        <p className="mt-1 text-content-secondary text-xs">Determines access level.</p>
      </div>
    </div>
  ),
}

export const AllSizes: Story = {
  render: () => (
    <div className="max-w-xs space-y-3">
      <Select selectSize="sm">
        <option>Small select</option>
      </Select>
      <Select selectSize="md">
        <option>Medium select</option>
      </Select>
    </div>
  ),
}

export const AllStates: Story = {
  render: () => (
    <div className="max-w-xs space-y-4">
      <div>
        <p className="mb-1 text-content-secondary text-xs">Default</p>
        <Select>
          <option>Option A</option>
          <option>Option B</option>
        </Select>
      </div>
      <div>
        <p className="mb-1 text-content-secondary text-xs">Disabled</p>
        <Select disabled>
          <option>Cannot change</option>
        </Select>
      </div>
      <div>
        <p className="mb-1 text-content-secondary text-xs">Many options</p>
        <Select>
          <option value="1">Option 1</option>
          <option value="2">Option 2</option>
          <option value="3">Option 3</option>
          <option value="4">Option 4</option>
          <option value="5">Option 5</option>
          <option value="6">Option 6</option>
          <option value="7">Option 7</option>
          <option value="8">Option 8</option>
          <option value="9">Option 9</option>
          <option value="10">Option 10</option>
        </Select>
      </div>
    </div>
  ),
}
