import { invoke } from '@tauri-apps/api/core'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it } from 'vitest'

describe('CRT-MK-M052: get_desktop_permission_status IPC contract', () => {
  afterEach(() => clearMocks())

  it('M052: returns desktop permission snapshot', async () => {
    mockIPC((cmd) => {
      if (cmd === 'get_desktop_permission_status') {
        return {
          platform: 'macos',
          accessibility: {
            state: 'needs_attention',
            status_reason: 'macos_accessibility_missing',
          },
          screen_capture: {
            state: 'granted',
            status_reason: 'macos_screen_capture_granted',
          },
          notifications: {
            state: 'needs_attention',
            status_reason: 'macos_notifications_not_determined',
          },
        }
      }
    })

    const result = await invoke<{
      platform: string
      accessibility: { state: string; status_reason: string | null }
      screen_capture: { state: string; status_reason: string | null }
      notifications: { state: string; status_reason: string | null }
    }>('get_desktop_permission_status')

    expect(result.platform).toBe('macos')
    expect(result.accessibility.state).toBe('needs_attention')
    expect(result.screen_capture.state).toBe('granted')
    expect(result.notifications.state).toBe('needs_attention')
  })
})
