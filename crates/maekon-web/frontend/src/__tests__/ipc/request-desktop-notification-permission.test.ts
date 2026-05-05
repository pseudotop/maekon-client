import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { requestDesktopNotificationPermission } from '../../api/client'

describe('request_desktop_notification_permission IPC contract', () => {
  afterEach(() => clearMocks())

  it('returns an updated desktop permission snapshot', async () => {
    const invokeSpy = vi.fn()

    mockIPC((cmd) => {
      invokeSpy(cmd)
      if (cmd === 'request_desktop_notification_permission') {
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
          notifications: {
            state: 'granted',
            status_reason: 'macos_notifications_granted',
          },
        }
      }
    })

    const result = await requestDesktopNotificationPermission()

    expect(invokeSpy).toHaveBeenCalledWith('request_desktop_notification_permission')
    expect(result.notifications.state).toBe('granted')
    expect(result.notifications.status_reason).toBe('macos_notifications_granted')
  })
})
