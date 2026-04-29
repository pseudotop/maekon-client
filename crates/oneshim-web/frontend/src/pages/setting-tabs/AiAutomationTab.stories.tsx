import type { Meta, StoryObj } from '@storybook/react'
import AiAutomationTab from './ai-automation'

const meta = {
  title: 'Settings/AiAutomationTab',
  component: AiAutomationTab,
  tags: ['autodocs'],
} satisfies Meta<typeof AiAutomationTab>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {}
