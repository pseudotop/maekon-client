import { screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import type { Frame } from '../../api/client'
import AllFrames from './AllFrames'
import type { TimelineContext } from './TimelineLayout'

const mockUseTypedOutletContext = vi.fn()

vi.mock('../../routes', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../routes')>()
  return {
    ...actual,
    useTypedOutletContext: () => mockUseTypedOutletContext(),
  }
})

const frame: Frame = {
  id: 42,
  timestamp: '2026-04-27T09:00:00.000Z',
  trigger_type: 'interval',
  app_name: 'Figma',
  window_title: 'Design Review',
  importance: 0.82,
  resolution: '1920x1080',
  file_path: null,
  ocr_text: null,
  image_url: '/frames/42.png',
  tag_ids: [],
}

function createContext(): TimelineContext {
  return {
    frames: [frame],
    filteredFrames: [frame],
    pagination: undefined,
    page: 0,
    setPage: vi.fn(),
    pageSize: 50,
    allTags: [],
    selectedFrame: null,
    setSelectedFrame: vi.fn(),
    selectedIndex: -1,
    setSelectedIndex: vi.fn(),
    selectedFrameTags: [],
    addTagMutation: {} as TimelineContext['addTagMutation'],
    removeTagMutation: {} as TimelineContext['removeTagMutation'],
    batchTagMutation: {} as TimelineContext['batchTagMutation'],
    viewMode: 'grid',
    setViewMode: vi.fn(),
    appFilter: 'all',
    setAppFilter: vi.fn(),
    importanceFilter: 'all',
    setImportanceFilter: vi.fn(),
    tagFilter: 'all',
    setTagFilter: vi.fn(),
    appList: ['Figma'],
    selectMode: false,
    setSelectMode: vi.fn(),
    selectedFrames: new Set(),
    setSelectedFrames: vi.fn(),
    toggleFrameSelection: vi.fn(),
    exitSelectMode: vi.fn(),
    selectAllFiltered: vi.fn(),
    selectFrame: vi.fn(),
    goToPrev: vi.fn(),
    goToNext: vi.fn(),
    openLightbox: vi.fn(),
    handleCopyOcr: vi.fn(),
    lightboxOpen: false,
    setLightboxOpen: vi.fn(),
    standaloneMode: false,
    captureEnabled: true,
  }
}

describe('AllFrames', () => {
  beforeEach(() => {
    mockUseTypedOutletContext.mockReturnValue(createContext())
  })

  it('gives grid thumbnails a descriptive accessible name', () => {
    renderWithProviders(<AllFrames />)

    expect(screen.getByRole('button', { name: /Figma.*Design Review.*82%/i })).toBeInTheDocument()
  })
})
