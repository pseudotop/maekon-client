import type { Meta, StoryObj } from '@storybook/react'
import { AppMemoryRouter } from '../../router/future'
import TitleBar from './TitleBar'

const meta = {
  title: 'Shell/TitleBar',
  component: TitleBar,
  tags: ['autodocs'],
  decorators: [
    (Story) => (
      <AppMemoryRouter initialEntries={['/']}>
        <Story />
      </AppMemoryRouter>
    ),
  ],
} satisfies Meta<typeof TitleBar>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    onSearchOpen: () => {},
  },
}

export const TimelinePage: Story = {
  args: {
    onSearchOpen: () => {},
  },
  decorators: [
    (Story) => (
      <AppMemoryRouter initialEntries={['/timeline']}>
        <Story />
      </AppMemoryRouter>
    ),
  ],
}

export const SettingsPage: Story = {
  args: {
    onSearchOpen: () => {},
  },
  decorators: [
    (Story) => (
      <AppMemoryRouter initialEntries={['/settings']}>
        <Story />
      </AppMemoryRouter>
    ),
  ],
}
