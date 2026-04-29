import type { Meta, StoryObj } from '@storybook/react'
import Lightbox from './Lightbox'

const noop = () => {}

const meta = {
  title: 'Domain Components/Lightbox',
  component: Lightbox,
  tags: ['autodocs'],
  args: {
    onClose: noop,
  },
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof Lightbox>

export default meta
type Story = StoryObj<typeof meta>

export const SingleImage: Story = {
  args: {
    imageUrl: 'https://placehold.co/1200x800/1e293b/e2e8f0?text=Screenshot+1',
    alt: 'Screenshot of application',
    hasPrev: false,
    hasNext: false,
  },
}

export const MultipleImages: Story = {
  args: {
    imageUrl: 'https://placehold.co/1200x800/1e293b/e2e8f0?text=Screenshot+2+of+5',
    alt: 'Screenshot 2 of 5',
    hasPrev: true,
    hasNext: true,
    onPrev: noop,
    onNext: noop,
  },
}
