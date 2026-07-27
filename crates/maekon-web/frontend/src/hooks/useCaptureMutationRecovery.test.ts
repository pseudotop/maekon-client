import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useCaptureMutationRecovery } from './useCaptureMutationRecovery'

const { addToast, requestCaptureReauth } = vi.hoisted(() => ({
  addToast: vi.fn(),
  requestCaptureReauth: vi.fn(),
}))

vi.mock('../components/CaptureReauthGate', () => ({
  useCaptureReauthRecovery: () => ({ requestCaptureReauth }),
}))

vi.mock('./useToast', () => ({ addToast }))

describe('useCaptureMutationRecovery', () => {
  beforeEach(() => {
    addToast.mockReset()
    requestCaptureReauth.mockReset()
  })

  it('surfaces a non-reauth mutation error', async () => {
    requestCaptureReauth.mockResolvedValue(false)
    const { result } = renderHook(() => useCaptureMutationRecovery('Update failed'))

    await act(() => result.current(new Error('network error'), vi.fn()))

    expect(addToast).toHaveBeenCalledWith('error', 'Update failed')
  })

  it('surfaces an error when the post-authentication retry still fails', async () => {
    let retryAfterAuthentication: (() => Promise<void>) | undefined
    requestCaptureReauth.mockImplementation(async (_error, retry) => {
      retryAfterAuthentication = retry
      return true
    })
    const { result } = renderHook(() => useCaptureMutationRecovery('Update failed'))

    await act(() => result.current(new Error('reauth required'), async () => Promise.reject(new Error('still failed'))))
    await act(async () => retryAfterAuthentication?.())

    expect(addToast).toHaveBeenCalledWith('error', 'Update failed')
  })
})
