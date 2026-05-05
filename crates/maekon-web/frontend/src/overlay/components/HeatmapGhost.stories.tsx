import type { Meta, StoryObj } from '@storybook/react'
import HeatmapGhost from './HeatmapGhost'

const meta = {
  title: 'Overlay/HeatmapGhost',
  component: HeatmapGhost,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof HeatmapGhost>

export default meta
type Story = StoryObj<typeof meta>

/**
 * HeatmapGhost renders a full-screen canvas and listens for Tauri
 * `overlay:heatmap-update` events. In Storybook (no Tauri runtime),
 * the canvas is blank.
 */
export const Default: Story = {}
