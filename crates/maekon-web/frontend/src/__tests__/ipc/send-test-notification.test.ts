import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { sendTestNotification } from '../../api/client'

describe('send_test_notification IPC contract', () => {
  afterEach(() => clearMocks())

  it('forwards localized title and body to the native notification command', async () => {
    const invokeSpy = vi.fn()

    mockIPC((cmd, args) => {
      invokeSpy(cmd, args)
      if (cmd === 'send_test_notification') {
        return { delivered: true }
      }
    })

    await expect(
      sendTestNotification({
        title: 'Maekon test notification',
        body: 'Notifications are ready.',
      }),
    ).resolves.toEqual({ delivered: true })

    expect(invokeSpy).toHaveBeenCalledWith('send_test_notification', {
      title: 'Maekon test notification',
      body: 'Notifications are ready.',
    })
  })
})
