import type { Meta, StoryObj } from '@storybook/react'
import { Card, CardContent, CardHeader, CardTitle } from './Card'

const meta = {
  title: 'UI Primitives/Card',
  component: Card,
  tags: ['autodocs'],
  argTypes: {
    variant: {
      control: 'select',
      options: ['default', 'elevated', 'highlight', 'interactive', 'danger'],
    },
    padding: {
      control: 'select',
      options: ['none', 'sm', 'md', 'lg'],
    },
  },
} satisfies Meta<typeof Card>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  render: (args) => (
    <Card {...args}>
      <CardHeader>
        <CardTitle>Card Title</CardTitle>
      </CardHeader>
      <CardContent>
        <p className="text-content-secondary text-sm">Card content goes here.</p>
      </CardContent>
    </Card>
  ),
  args: { variant: 'default', padding: 'md' },
}

export const Elevated: Story = {
  render: (args) => (
    <Card {...args}>
      <CardHeader>
        <CardTitle>Elevated Card</CardTitle>
      </CardHeader>
      <CardContent>
        <p className="text-content-secondary text-sm">Muted surface variant.</p>
      </CardContent>
    </Card>
  ),
  args: { variant: 'elevated' },
}

export const Highlight: Story = {
  render: (args) => (
    <Card {...args}>
      <CardHeader>
        <CardTitle>Highlight Card</CardTitle>
      </CardHeader>
      <CardContent>
        <p className="text-content-secondary text-sm">Gradient highlight variant.</p>
      </CardContent>
    </Card>
  ),
  args: { variant: 'highlight' },
}

export const Interactive: Story = {
  render: (args) => (
    <Card {...args}>
      <CardHeader>
        <CardTitle>Interactive Card</CardTitle>
      </CardHeader>
      <CardContent>
        <p className="text-content-secondary text-sm">Hover to see interaction.</p>
      </CardContent>
    </Card>
  ),
  args: { variant: 'interactive' },
}

export const DangerCard: Story = {
  render: (args) => (
    <Card {...args}>
      <CardHeader>
        <CardTitle>Danger Card</CardTitle>
      </CardHeader>
      <CardContent>
        <p className="text-semantic-error text-sm">Something went wrong.</p>
      </CardContent>
    </Card>
  ),
  args: { variant: 'danger' },
}

export const AllPaddings: Story = {
  render: () => (
    <div className="space-y-4">
      {(['none', 'sm', 'md', 'lg'] as const).map((p) => (
        <Card key={p} padding={p}>
          <CardContent>
            <p className="text-content-secondary text-sm">padding=&quot;{p}&quot;</p>
          </CardContent>
        </Card>
      ))}
    </div>
  ),
}
