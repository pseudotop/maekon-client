import type { Meta, StoryObj } from '@storybook/react'
import { addToast, clearToasts } from '../../hooks/useToast'
import { Button } from './Button'
import { ToastContainer } from './Toast'

function ToastDemo() {
  return (
    <div className="min-h-[16rem] bg-surface-base p-6">
      <div className="flex flex-wrap gap-3">
        <Button
          onClick={() => {
            clearToasts()
            addToast('success', 'Settings saved. Some settings require app restart.', 0)
          }}
        >
          Success Toast
        </Button>
        <Button
          variant="secondary"
          onClick={() => {
            clearToasts()
            addToast('info', 'Checking for updates…', 0)
          }}
        >
          Info Toast
        </Button>
        <Button
          variant="ghost"
          onClick={() => {
            clearToasts()
            addToast('warning', 'Model discovery returned no selectable OCR models.', 0)
          }}
        >
          Warning Toast
        </Button>
        <Button
          variant="danger"
          onClick={() => {
            clearToasts()
            addToast('error', 'Failed to save settings: API key is missing.', 0)
          }}
        >
          Error Toast
        </Button>
      </div>

      <ToastContainer />
    </div>
  )
}

const meta = {
  title: 'UI Primitives/Toast',
  component: ToastContainer,
  tags: ['autodocs'],
  render: () => <ToastDemo />,
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof ToastContainer>

export default meta
type Story = StoryObj<typeof meta>

export const Playground: Story = {}

export const AllTypes: Story = {
  render: () => {
    function AllTypesDemo() {
      return (
        <div className="min-h-[20rem] bg-surface-base p-6">
          <Button
            onClick={() => {
              clearToasts()
              addToast('success', 'Settings saved. Some settings require app restart.', 0)
              addToast('info', 'Checking for updates…', 0)
              addToast('warning', 'Model discovery returned no selectable OCR models.', 0)
              addToast('error', 'Failed to save settings: API key is missing.', 0)
            }}
          >
            Show All Types
          </Button>
          <ToastContainer />
        </div>
      )
    }
    return <AllTypesDemo />
  },
}

export const AutoDismiss: Story = {
  render: () => {
    function AutoDismissDemo() {
      return (
        <div className="min-h-[16rem] bg-surface-base p-6">
          <p className="mb-3 text-content-secondary text-xs">Toasts auto-dismiss after 4 seconds (default duration).</p>
          <Button
            onClick={() => {
              addToast('info', 'This toast will disappear in 4 seconds.')
            }}
          >
            Show Auto-Dismiss Toast
          </Button>
          <ToastContainer />
        </div>
      )
    }
    return <AutoDismissDemo />
  },
}

export const Stacked: Story = {
  render: () => {
    function StackedDemo() {
      return (
        <div className="min-h-[24rem] bg-surface-base p-6">
          <div className="flex flex-wrap gap-3">
            <Button
              onClick={() => {
                addToast('success', `Task completed at ${new Date().toLocaleTimeString()}`, 0)
              }}
            >
              Add Toast
            </Button>
            <Button variant="secondary" onClick={() => clearToasts()}>
              Clear All
            </Button>
          </div>
          <ToastContainer />
        </div>
      )
    }
    return <StackedDemo />
  },
}
