import { renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useFrameImage } from './useFrameImage'

const { fetchMock, resolveFrameImageRequestUrl, withResolvedLocalAuthHeaders } = vi.hoisted(() => ({
  fetchMock: vi.fn(),
  resolveFrameImageRequestUrl: vi.fn(),
  withResolvedLocalAuthHeaders: vi.fn(),
}))

vi.mock('../utils/platform', () => ({ IS_TAURI: true }))
vi.mock('../utils/api-base', () => ({
  // Positive control: the synchronous fallback deliberately points at the
  // wrong coexisting instance. The authenticated fetch must not use it.
  resolveImageUrl: () => 'http://127.0.0.1:10090/api/frames/42/image',
  resolveFrameImageRequestUrl,
  withResolvedLocalAuthHeaders,
}))
vi.mock('../api/reauth', () => ({ authenticateCaptureHistory: vi.fn() }))

describe('useFrameImage', () => {
  beforeEach(() => {
    fetchMock.mockReset()
    resolveFrameImageRequestUrl.mockReset()
    withResolvedLocalAuthHeaders.mockReset()
    vi.stubGlobal('fetch', fetchMock)
    Object.defineProperty(URL, 'createObjectURL', {
      configurable: true,
      value: vi.fn(() => 'blob:frame-42'),
    })
    Object.defineProperty(URL, 'revokeObjectURL', {
      configurable: true,
      value: vi.fn(),
    })
  })

  it('fetches through the live instance port with local authentication', async () => {
    resolveFrameImageRequestUrl.mockResolvedValue('http://127.0.0.1:10091/api/frames/42/image')
    withResolvedLocalAuthHeaders.mockResolvedValue({
      method: 'GET',
      headers: { 'X-Local-Auth': 'dev-instance-token' },
    })
    fetchMock.mockResolvedValue(
      new Response(new Blob(['frame-bytes'], { type: 'image/webp' }), {
        status: 200,
        headers: { 'Content-Type': 'image/webp' },
      }),
    )

    const { result } = renderHook(() => useFrameImage('/api/frames/42/image'))

    await waitFor(() => expect(result.current.phase).toBe('ready'))
    expect(resolveFrameImageRequestUrl).toHaveBeenCalledWith('/api/frames/42/image')
    expect(fetchMock).toHaveBeenCalledWith('http://127.0.0.1:10091/api/frames/42/image', {
      method: 'GET',
      headers: { 'X-Local-Auth': 'dev-instance-token' },
    })
    expect(result.current.src).toBe('blob:frame-42')
  })
})
