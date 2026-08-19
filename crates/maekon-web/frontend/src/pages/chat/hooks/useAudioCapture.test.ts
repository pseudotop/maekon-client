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
const mockConsentState = vi.hoisted(() => ({ microphone: true }))

// dynamic import mock for @tauri-apps/api/event (vi.mock is hoisted)
vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: Parameters<typeof mockListen>) => mockListen(...args),
}))

// #8053: force macOS so the mic-permission guidance path is deterministic. Harmless
// to the other tests, which do not read the platform flags.
vi.mock('../../../utils/platform', () => ({
  IS_MAC: true,
  IS_WINDOWS: false,
  IS_LINUX: false,
  IS_TAURI: false,
  isTauriRuntime: () => false,
  MOD_KEY: 'Ctrl',
}))

// @tauri-apps/api/core invoke mock — returns granted microphone consent and
// get_audio_status in voice_activity mode.
// #7600: get_feature_capabilities is checked FIRST (COMPILE-capability gate) — must
// resolve audio_compiled=true here or the hook short-circuits before reaching
// get_audio_status, and micMode never flips to 'voice_activity' (the VAD listener
// registration below depends on that transition).
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'get_feature_capabilities') {
      return { features: [], audio_compiled: true }
    }
    if (cmd === 'get_audio_status') {
      return {
        enabled: true,
        model_status: { state: 'ready' },
        stt_provider_loaded: true,
        mic_input_mode: 'voice_activity',
      }
    }
    if (cmd === 'get_consent') {
      return {
        status: 'Valid',
        permissions: { microphone: mockConsentState.microphone },
      }
    }
    // #8053: simulate a microphone permission denial for the guidance test.
    if (cmd === 'start_vad_listening' || cmd === 'start_audio_capture') {
      throw new Error('microphone access was not granted')
    }
    return undefined
  }),
}))

describe('useAudioCapture — privacy_gate_closed toast', () => {
  let addToastSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    mockConsentState.microphone = true
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

describe('useAudioCapture — mic permission guidance (#8053)', () => {
  let addToastSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    mockConsentState.microphone = true
    capturedListeners.clear()
    mockListen.mockClear()
    mockUnlisten.mockClear()
    addToastSpy = vi.spyOn(useToastModule, 'addToast')
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('마이크 시작 실패 시 OS별 권한 안내를 토스트에 포함한다', async () => {
    const { useAudioCapture } = await import('./useAudioCapture')
    const setInput = vi.fn() as React.Dispatch<React.SetStateAction<string>>
    const { result } = renderHook(() => useAudioCapture(false, setInput))
    await waitFor(() => expect(capturedListeners.has('vad-state-changed')).toBe(true))

    // start_vad_listening rejects (permission denied) → the error toast must carry
    // OS-aware guidance for 8s. IS_MAC is forced true, so the macOS path appears.
    await act(async () => {
      await result.current.handleVadToggle()
    })

    expect(addToastSpy).toHaveBeenCalledWith('error', expect.stringContaining('System Settings'), 8000)
    expect(addToastSpy).toHaveBeenCalledWith('error', expect.stringContaining('If microphone access was blocked'), 8000)
  })
})

describe('useAudioCapture — microphone consent readiness', () => {
  beforeEach(() => {
    mockConsentState.microphone = true
    capturedListeners.clear()
    mockListen.mockClear()
    mockUnlisten.mockClear()
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('disables the microphone affordance when microphone consent is absent', async () => {
    mockConsentState.microphone = false
    const { useAudioCapture } = await import('./useAudioCapture')
    const setInput = vi.fn() as React.Dispatch<React.SetStateAction<string>>
    const { result } = renderHook(() => useAudioCapture(false, setInput))

    await waitFor(() => expect(result.current.audioAvailable).toBe(false))

    expect(result.current.audioTooltip).toContain('consent')
    expect(capturedListeners.has('vad-state-changed')).toBe(false)
  })
})
