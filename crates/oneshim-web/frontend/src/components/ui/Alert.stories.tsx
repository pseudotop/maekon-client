import type { Meta, StoryObj } from '@storybook/react'
import { AlertCircle, CheckCircle, Info, TriangleAlert } from 'lucide-react'
import { Alert } from './Alert'

const meta = {
  title: 'UI Primitives/Alert',
  component: Alert,
  tags: ['autodocs'],
  argTypes: {
    variant: {
      control: 'select',
      options: ['default', 'info', 'success', 'warning', 'error'],
    },
  },
} satisfies Meta<typeof Alert>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    title: 'Note',
    children: 'This is a default informational alert.',
  },
}

export const InfoVariant: Story = {
  args: {
    variant: 'info',
    icon: <Info className="h-5 w-5" />,
    title: 'Information',
    children: 'Your data is synced every 10 seconds.',
  },
}

export const Success: Story = {
  args: {
    variant: 'success',
    icon: <CheckCircle className="h-5 w-5" />,
    title: 'Success',
    children: 'Settings saved successfully.',
  },
}

export const Warning: Story = {
  args: {
    variant: 'warning',
    icon: <TriangleAlert className="h-5 w-5" />,
    title: 'Warning',
    children: 'Storage usage exceeds 80%.',
  },
}

export const ErrorVariant: Story = {
  args: {
    variant: 'error',
    icon: <AlertCircle className="h-5 w-5" />,
    title: 'Error',
    children: 'Connection to server failed.',
  },
}

export const AllVariants: Story = {
  render: () => (
    <div className="max-w-md space-y-3">
      <Alert variant="default" title="Default">
        Neutral message.
      </Alert>
      <Alert variant="info" icon={<Info className="h-5 w-5" />} title="Info">
        Informational message.
      </Alert>
      <Alert variant="success" icon={<CheckCircle className="h-5 w-5" />} title="Success">
        Something worked.
      </Alert>
      <Alert variant="warning" icon={<TriangleAlert className="h-5 w-5" />} title="Warning">
        Be careful.
      </Alert>
      <Alert variant="error" icon={<AlertCircle className="h-5 w-5" />} title="Error">
        Something broke.
      </Alert>
    </div>
  ),
}
