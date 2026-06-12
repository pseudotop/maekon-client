import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { requestDesktopScreenCapturePermission } from '../../api/client'

describe('request_desktop_screen_capture_permission IPC contract', () => {
  afterEach(() => clearMocks())

  it('returns an updated desktop permission snapshot', async () => {
    const invokeSpy = vi.fn()

    mockIPC((cmd) => {
      invokeSpy(cmd)
      if (cmd === 'request_desktop_screen_capture_permission') {
        return {
          platform: 'macos',
          accessibility: {
            state: 'granted',
            status_reason: 'macos_accessibility_granted',
          },
          screen_capture: {
            state: 'granted',
            status_reason: 'macos_screen_capture_granted',
          },
          microphone: {
            state: 'needs_attention',
            status_reason: 'macos_microphone_not_determined',
          },
          input_monitoring: {
            state: 'needs_attention',
            status_reason: 'macos_input_monitoring_unknown',
          },
          notifications: {
            state: 'needs_attention',
            status_reason: 'macos_notifications_not_determined',
          },
        }
      }
    })

    const result = await requestDesktopScreenCapturePermission()

    expect(invokeSpy).toHaveBeenCalledWith('request_desktop_screen_capture_permission')
    expect(result.screen_capture.state).toBe('granted')
    expect(result.screen_capture.status_reason).toBe('macos_screen_capture_granted')
  })
})
