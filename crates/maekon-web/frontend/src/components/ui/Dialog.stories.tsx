import type { Meta, StoryObj } from '@storybook/react'
import { useState } from 'react'
import { Button } from './Button'
import { Dialog, DialogBody, DialogContent, DialogFooter, DialogTitle } from './Dialog'

function DialogDemo({ size = 'md' }: { size?: 'sm' | 'md' | 'lg' }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Button onClick={() => setOpen(true)}>Open Dialog</Button>
      <Dialog open={open} onClose={() => setOpen(false)}>
        <DialogContent size={size}>
          <DialogTitle>Confirm Action</DialogTitle>
          <DialogBody>Are you sure you want to proceed? This action cannot be undone.</DialogBody>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button variant="primary" onClick={() => setOpen(false)}>
              Confirm
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}

const meta = {
  title: 'UI Primitives/Dialog',
  component: Dialog,
  tags: ['autodocs'],
  parameters: { layout: 'centered' },
} satisfies Meta<typeof Dialog>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  render: () => <DialogDemo />,
}

export const Small: Story = {
  render: () => <DialogDemo size="sm" />,
}

export const Large: Story = {
  render: () => <DialogDemo size="lg" />,
}

export const DangerConfirm: Story = {
  render: () => {
    const [open, setOpen] = useState(false)
    return (
      <>
        <Button variant="danger" onClick={() => setOpen(true)}>
          Delete Data
        </Button>
        <Dialog open={open} onClose={() => setOpen(false)}>
          <DialogContent size="sm">
            <DialogTitle>Delete All Data?</DialogTitle>
            <DialogBody>This will permanently remove all stored activity data. This cannot be undone.</DialogBody>
            <DialogFooter>
              <Button variant="ghost" onClick={() => setOpen(false)}>
                Cancel
              </Button>
              <Button variant="danger" onClick={() => setOpen(false)}>
                Delete
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </>
    )
  },
}
