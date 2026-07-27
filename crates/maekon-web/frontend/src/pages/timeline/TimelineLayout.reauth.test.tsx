import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, Route, Routes, useOutletContext } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import TimelineLayout, { type TimelineContext } from './TimelineLayout'

const {
  addTagToFrame,
  batchAddTag,
  fetchFrames,
  fetchFrameTags,
  fetchSettings,
  fetchTags,
  recoverMutation,
  removeTagFromFrame,
} = vi.hoisted(() => ({
  addTagToFrame: vi.fn(),
  batchAddTag: vi.fn(),
  fetchFrames: vi.fn(),
  fetchFrameTags: vi.fn(),
  fetchSettings: vi.fn(),
  fetchTags: vi.fn(),
  recoverMutation: vi.fn(),
  removeTagFromFrame: vi.fn(),
}))

vi.mock('../../api/client', () => ({
  addTagToFrame,
  batchAddTag,
  fetchFrames,
  fetchFrameTags,
  fetchSettings,
  fetchTags,
  removeTagFromFrame,
}))

vi.mock('../../api/standalone', () => ({ isStandaloneModeEnabled: () => false }))
vi.mock('../../components/DateRangePicker', () => ({ default: () => null }))
vi.mock('../../components/Lightbox', () => ({ default: () => null }))
vi.mock('../../hooks/useCaptureMutationRecovery', () => ({
  useCaptureMutationRecovery: () => recoverMutation,
}))
vi.mock('../../hooks/useKeyboardShortcuts', () => ({ useKeyboardShortcuts: () => undefined }))
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}))

function TagMutationProbe() {
  const { removeTagMutation } = useOutletContext<TimelineContext>()
  return (
    <button type="button" onClick={() => removeTagMutation.mutate({ frameId: 4, tagId: 7 })}>
      remove tag
    </button>
  )
}

describe('TimelineLayout capture re-auth recovery', () => {
  beforeEach(() => {
    addTagToFrame.mockReset()
    batchAddTag.mockReset()
    fetchFrames.mockReset()
    fetchFrameTags.mockReset()
    fetchSettings.mockReset()
    fetchTags.mockReset()
    recoverMutation.mockReset()
    removeTagFromFrame.mockReset()

    fetchFrames.mockResolvedValue({ data: [], pagination: { total: 0, has_more: false } })
    fetchFrameTags.mockResolvedValue([])
    fetchSettings.mockResolvedValue({ capture_enabled: true })
    fetchTags.mockResolvedValue([])
  })

  it('routes an expired tag removal through re-auth recovery and provides an exact retry', async () => {
    const user = userEvent.setup()
    const expired = new Error('reauth required')
    removeTagFromFrame.mockRejectedValueOnce(expired).mockResolvedValueOnce(undefined)
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })

    render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={['/timeline']}>
          <Routes>
            <Route path="timeline" element={<TimelineLayout />}>
              <Route index element={<TagMutationProbe />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    )

    await user.click(await screen.findByRole('button', { name: 'remove tag' }))
    await waitFor(() => expect(recoverMutation).toHaveBeenCalledWith(expired, expect.any(Function)))

    const retry = recoverMutation.mock.calls[0][1] as () => Promise<void>
    await retry()

    expect(removeTagFromFrame).toHaveBeenNthCalledWith(1, 4, 7)
    expect(removeTagFromFrame).toHaveBeenNthCalledWith(2, 4, 7)
  })
})
