import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { FrameAnnotations } from './FrameAnnotations'

const { createFrameAnnotation, deleteFrameAnnotation, fetchFrameAnnotations, recoverMutation } = vi.hoisted(() => ({
  createFrameAnnotation: vi.fn(),
  deleteFrameAnnotation: vi.fn(),
  fetchFrameAnnotations: vi.fn(),
  recoverMutation: vi.fn(),
}))

vi.mock('../../api/client', () => ({
  createFrameAnnotation,
  deleteFrameAnnotation,
  fetchFrameAnnotations,
}))

vi.mock('../../hooks/useCaptureMutationRecovery', () => ({
  useCaptureMutationRecovery: () => recoverMutation,
}))

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { resolvedLanguage: 'en', language: 'en' },
  }),
}))

describe('FrameAnnotations', () => {
  beforeEach(() => {
    createFrameAnnotation.mockReset()
    deleteFrameAnnotation.mockReset()
    fetchFrameAnnotations.mockReset()
    recoverMutation.mockReset()
    fetchFrameAnnotations.mockResolvedValue([
      {
        annotation_id: 'note-1',
        frame_id: 4,
        annotation_type: 'Memo',
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        color: null,
        text: 'Temporary note',
        created_at: '2026-07-15T00:00:00Z',
      },
    ])
  })

  it('routes an expired delete through re-auth recovery and provides an exact retry', async () => {
    const user = userEvent.setup()
    const expired = new Error('reauth required')
    deleteFrameAnnotation.mockRejectedValueOnce(expired).mockResolvedValueOnce(undefined)
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })

    render(
      <QueryClientProvider client={queryClient}>
        <FrameAnnotations frameId={4} />
      </QueryClientProvider>,
    )

    await user.click(await screen.findByRole('button', { name: 'timeline.deleteAnnotation' }))
    await waitFor(() => expect(recoverMutation).toHaveBeenCalledWith(expired, expect.any(Function)))

    const retry = recoverMutation.mock.calls[0][1] as () => Promise<void>
    await retry()

    expect(deleteFrameAnnotation).toHaveBeenNthCalledWith(1, 4, 'note-1')
    expect(deleteFrameAnnotation).toHaveBeenNthCalledWith(2, 4, 'note-1')
  })
})
