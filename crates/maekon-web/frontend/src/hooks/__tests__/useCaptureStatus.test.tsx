import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { CaptureStatus } from '../useCaptureStatus'
import { useCaptureStatus } from '../useCaptureStatus'

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  unlisten: vi.fn(),
  handler: null as ((event: { payload: CaptureStatus }) => void) | null,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }))

const active: CaptureStatus = {
  paused: false,
  indicator_visible: true,
  consent_granted: true,
  permitted: true,
}

describe('useCaptureStatus', () => {
  beforeEach(() => {
    mocks.invoke.mockReset()
    mocks.listen.mockReset()
    mocks.unlisten.mockReset()
    mocks.handler = null
  })

  it('reads the desktop state and stays synchronized with capture events', async () => {
    mocks.invoke.mockResolvedValue(active)
    mocks.listen.mockImplementation(async (_event: string, handler: (event: { payload: CaptureStatus }) => void) => {
      mocks.handler = handler
      return mocks.unlisten
    })

    const { result, unmount } = renderHook(() => useCaptureStatus())

    await waitFor(() => expect(result.current).toEqual(active))
    expect(mocks.invoke).toHaveBeenCalledWith('get_capture_status')
    expect(mocks.listen).toHaveBeenCalledWith('overlay:capture-state-changed', expect.any(Function))

    const paused = { ...active, paused: true, permitted: false }
    act(() => mocks.handler?.({ payload: paused }))
    expect(result.current).toEqual(paused)

    unmount()
    expect(mocks.unlisten).toHaveBeenCalledOnce()
  })

  it('remains fail-closed when desktop IPC is unavailable', async () => {
    mocks.invoke.mockRejectedValue(new Error('not running in Tauri'))

    const { result } = renderHook(() => useCaptureStatus())

    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith('get_capture_status'))
    expect(result.current).toBeNull()
    expect(mocks.listen).not.toHaveBeenCalled()
  })
})
