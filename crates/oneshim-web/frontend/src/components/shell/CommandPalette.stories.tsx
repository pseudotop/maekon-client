import type { Meta, StoryObj } from '@storybook/react'
import { ThemeProvider } from '../../contexts/ThemeContext'
import { AppMemoryRouter } from '../../router/future'
import CommandPalette from './CommandPalette'

const meta = {
  title: 'Shell/CommandPalette',
  component: CommandPalette,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
  decorators: [
    (Story) => (
      <ThemeProvider>
        <AppMemoryRouter initialEntries={['/']}>
          <Story />
        </AppMemoryRouter>
      </ThemeProvider>
    ),
  ],
} satisfies Meta<typeof CommandPalette>

export default meta
type Story = StoryObj<typeof meta>

export const Open: Story = {
  args: {
    isOpen: true,
    onClose: () => {},
    onToggleSidebar: () => {},
  },
}

export const Closed: Story = {
  args: {
    isOpen: false,
    onClose: () => {},
    onToggleSidebar: () => {},
  },
}
