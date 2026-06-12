import { screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { FrameCard } from './SessionPlayback'
import type { PlaybackState } from './types'

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>()
  return {
    ...actual,
    fetchFrameTags: vi.fn(async () => []),
  }
})

function makePlayback(): PlaybackState {
  const frameTime = new Date('2026-05-16T04:00:00.000Z')
  return {
    isPlaying: false,
    playbackSpeed: 1,
    currentTime: frameTime,
    startTime: frameTime,
    endTime: frameTime,
    currentFrame: {
      type: 'Frame',
      id: 42,
      timestamp: frameTime.toISOString(),
      app_name: 'Notes',
      window_title: 'meeting notes',
      importance: 0.9,
      image_url: '/api/frames/42/image',
      ocr_text: 'follow up with Alex at 3pm',
    },
    handlePlayPause: vi.fn(),
    handleSpeedChange: vi.fn(),
    handleTimeChange: vi.fn(),
    handleSkipToStart: vi.fn(),
    handleSkipToEnd: vi.fn(),
  }
}

describe('FrameCard', () => {
  it('shows OCR text captured with the replay frame', () => {
    renderWithProviders(
      <FrameCard
        playback={makePlayback()}
        viewportSlot={<img alt="Captured frame" src="/api/frames/42/image" />}
        statusSlot={null}
      />,
    )

    expect(screen.getByText('Captured OCR')).toBeInTheDocument()
    expect(screen.getByText('follow up with Alex at 3pm')).toBeInTheDocument()
  })
})
