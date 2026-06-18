// Unit test verifying that addToast is called when a privacy_gate_closed event is received
import { act, renderHook, waitFor } from '@testing-library/react'
import type React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as useToastModule from '../../../hooks/useToast'

// mock for capturing the vad-state-changed listener callback
const capturedListeners = new Map<string, (event: { payload: unknown }) => void>()
const mockUnlisten = vi.fn()
const mockListen = vi.fn(async (eventName: string, cb: (event: { payload: unknown }) => void) => {
  capturedListeners.set(eventName, cb)
  return mockUnlisten
})

// dynamic import mock for @tauri-apps/api/event (vi.mock is hoisted)
vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: Parameters<typeof mockListen>) => mockListen(...args),
}))

// @tauri-apps/api/core invoke mock — returns get_audio_status in voice_activity mode
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'get_audio_status') {
      return {
        enabled: true,
        model_status: { state: 'ready' },
        stt_provider_loaded: true,
        mic_input_mode: 'voice_activity',
      }
    }
    return undefined
  }),
}))

describe('useAudioCapture — privacy_gate_closed toast', () => {
  let addToastSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    capturedListeners.clear()
    mockListen.mockClear()
    mockUnlisten.mockClear()
    // replace addToast with a spy (keeps the real implementation; purpose is to verify the call)
    addToastSpy = vi.spyOn(useToastModule, 'addToast')
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  async function setupHook() {
    const { useAudioCapture } = await import('./useAudioCapture')
    const setInput = vi.fn() as React.Dispatch<React.SetStateAction<string>>
    const { result } = renderHook(() => useAudioCapture(false, setInput))
    // wait until the VAD listener registration completes
    await waitFor(() => expect(capturedListeners.has('vad-state-changed')).toBe(true))
    return result
  }

  it('privacy_gate_closed reason 수신 시 warning 토스트를 표시한다', async () => {
    await setupHook()

    const vadCallback = capturedListeners.get('vad-state-changed')!
    act(() => {
      vadCallback({ payload: { state: 'idle', reason: 'privacy_gate_closed' } })
    })

    expect(addToastSpy).toHaveBeenCalledWith('warning', expect.stringContaining('privacy gate'), 5000)
  })

  it('reason 없이 idle 상태 수신 시 토스트를 표시하지 않는다', async () => {
    await setupHook()

    const vadCallback = capturedListeners.get('vad-state-changed')!
    act(() => {
      vadCallback({ payload: { state: 'idle' } })
    })

    expect(addToastSpy).not.toHaveBeenCalled()
  })

  it('다른 reason과 함께 idle 상태 수신 시 토스트를 표시하지 않는다', async () => {
    await setupHook()

    const vadCallback = capturedListeners.get('vad-state-changed')!
    act(() => {
      vadCallback({ payload: { state: 'idle', reason: 'user_stopped' } })
    })

    expect(addToastSpy).not.toHaveBeenCalled()
  })
})
