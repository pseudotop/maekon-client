import type { Meta, StoryObj } from '@storybook/react'
import ErrorBoundary from './ErrorBoundary'

const meta = {
  title: 'Domain Components/ErrorBoundary',
  component: ErrorBoundary,
  tags: ['autodocs'],
} satisfies Meta<typeof ErrorBoundary>

export default meta
type Story = StoryObj<typeof meta>

export const Normal: Story = {
  args: {
    children: (
      <div className="rounded-lg bg-surface-overlay p-6">
        <p className="text-content">This content renders normally inside the ErrorBoundary.</p>
      </div>
    ),
  },
}

function BrokenComponent(): never {
  throw new Error('Test error: something went wrong in the child component')
}

export const WithError: Story = {
  args: {
    children: <BrokenComponent />,
  },
}

export const WithFallback: Story = {
  args: {
    children: <BrokenComponent />,
    fallback: (
      <div className="rounded-lg border border-semantic-warning bg-semantic-warning/10 p-6 text-center">
        <p className="font-medium text-semantic-warning">Custom fallback: An error occurred.</p>
      </div>
    ),
  },
}
