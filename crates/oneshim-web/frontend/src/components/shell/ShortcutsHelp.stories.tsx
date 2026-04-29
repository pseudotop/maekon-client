import type { Meta, StoryObj } from '@storybook/react'
import ShortcutsHelp from './ShortcutsHelp'

const meta = {
  title: 'Shell/ShortcutsHelp',
  component: ShortcutsHelp,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof ShortcutsHelp>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    onClose: () => {},
  },
}
