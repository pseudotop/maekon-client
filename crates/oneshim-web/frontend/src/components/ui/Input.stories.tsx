import type { Meta, StoryObj } from '@storybook/react'
import { Input } from './Input'

const meta = {
  title: 'UI Primitives/Input',
  component: Input,
  tags: ['autodocs'],
  argTypes: {
    inputSize: {
      control: 'select',
      options: ['sm', 'md', 'lg'],
    },
    error: { control: 'boolean' },
    disabled: { control: 'boolean' },
  },
  args: {
    placeholder: 'Enter text...',
  },
} satisfies Meta<typeof Input>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: { inputSize: 'md' },
}

export const Small: Story = {
  args: { inputSize: 'sm', placeholder: 'Small input' },
}

export const Large: Story = {
  args: { inputSize: 'lg', placeholder: 'Large input' },
}

export const ErrorState: Story = {
  args: { error: true, placeholder: 'Invalid input' },
}

export const Disabled: Story = {
  args: { disabled: true, placeholder: 'Disabled input' },
}

export const WithLabel: Story = {
  render: () => (
    <div className="max-w-md space-y-4">
      <div>
        <label htmlFor="email" className="mb-2 block font-medium text-content-strong text-sm">
          Email address
        </label>
        <Input id="email" type="email" placeholder="you@example.com" />
      </div>
      <div>
        <label htmlFor="password" className="mb-2 block font-medium text-content-strong text-sm">
          Password
        </label>
        <Input id="password" type="password" placeholder="Enter password" />
        <p className="mt-1 text-content-secondary text-xs">Must be at least 8 characters.</p>
      </div>
    </div>
  ),
}

export const AllSizes: Story = {
  render: () => (
    <div className="max-w-md space-y-3">
      <Input inputSize="sm" placeholder="Small" />
      <Input inputSize="md" placeholder="Medium" />
      <Input inputSize="lg" placeholder="Large" />
    </div>
  ),
}

export const AllVariants: Story = {
  render: () => (
    <div className="max-w-md space-y-4">
      <div>
        <p className="mb-1 text-content-secondary text-xs">Default</p>
        <Input placeholder="Default input" />
      </div>
      <div>
        <p className="mb-1 text-content-secondary text-xs">Error</p>
        <Input error placeholder="Invalid value" />
      </div>
      <div>
        <p className="mb-1 text-content-secondary text-xs">Disabled</p>
        <Input disabled placeholder="Cannot edit" />
      </div>
      <div>
        <p className="mb-1 text-content-secondary text-xs">With value</p>
        <Input defaultValue="Prefilled text" />
      </div>
    </div>
  ),
}
