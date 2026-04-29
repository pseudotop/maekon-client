import type { Meta, StoryObj } from '@storybook/react'
import type { DetectionScenePayload } from '../types'
import DetectionOverlay from './DetectionOverlay'

const meta = {
  title: 'Overlay/DetectionOverlay',
  component: DetectionOverlay,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof DetectionOverlay>

export default meta
type Story = StoryObj<typeof meta>

const sampleScene: DetectionScenePayload = {
  scene_id: 'scene-1',
  app_name: 'VS Code',
  screen_width: 1920,
  screen_height: 1080,
  element_count: 4,
  elements: [
    {
      element_id: 'el-1',
      x: 50,
      y: 60,
      width: 200,
      height: 36,
      label: 'Search',
      role: 'AXTextField',
      confidence: 0.95,
      source: 'accessibility',
    },
    {
      element_id: 'el-2',
      x: 300,
      y: 60,
      width: 100,
      height: 32,
      label: 'Run',
      role: 'AXButton',
      confidence: 0.9,
      source: 'accessibility',
    },
    {
      element_id: 'el-3',
      x: 50,
      y: 120,
      width: 180,
      height: 28,
      label: 'File Explorer',
      role: 'AXLink',
      confidence: 0.85,
      source: 'vision',
    },
    {
      element_id: 'el-4',
      x: 300,
      y: 120,
      width: 150,
      height: 28,
      label: 'Settings',
      role: 'AXMenuItem',
      confidence: 0.88,
      source: 'accessibility',
    },
  ],
}

/** Detection boxes for multiple UI elements, no selection. */
export const Default: Story = {
  args: {
    scene: sampleScene,
    selectedId: null,
    onSelect: () => {},
  },
}

/** An element selected — shows inspector panel. */
export const WithSelection: Story = {
  args: {
    scene: sampleScene,
    selectedId: 'el-1',
    onSelect: () => {},
  },
}

/** Scene with a single element. */
export const SingleElement: Story = {
  args: {
    scene: {
      scene_id: 'scene-2',
      app_name: 'Safari',
      screen_width: 1440,
      screen_height: 900,
      element_count: 1,
      elements: [
        {
          element_id: 'el-solo',
          x: 120,
          y: 80,
          width: 300,
          height: 44,
          label: 'Address Bar',
          role: 'AXTextField',
          confidence: 0.97,
          source: 'accessibility',
        },
      ],
    },
    selectedId: null,
    onSelect: () => {},
  },
}
