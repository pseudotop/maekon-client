import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { withdrawConsent } from '../../api/client'

// withdraw_consent → ConsentSnapshot. After withdrawal, status is NotGranted and all permissions are false.
describe('withdraw_consent IPC contract', () => {
  afterEach(() => clearMocks())

  it('returns the NotGranted snapshot after withdrawal', async () => {
    const invokeSpy = vi.fn()

    mockIPC((cmd) => {
      invokeSpy(cmd)
      if (cmd === 'withdraw_consent') {
        return {
          status: 'NotGranted',
          permissions: {
            screen_capture: false,
            ocr_processing: false,
            telemetry: false,
            process_monitoring: false,
            input_activity: false,
            window_title_collection: false,
            app_usage_analytics: false,
            clipboard_monitoring: false,
            file_access_monitoring: false,
            activity_pattern_learning: false,
            cross_device_sync: false,
            full_text_extraction: false,
            memory_graph_enrichment: false,
            microphone: false,
            unredacted_external_ocr: false,
          },
        }
      }
    })

    const result = await withdrawConsent()

    expect(invokeSpy).toHaveBeenCalledWith('withdraw_consent')
    expect(result.status).toBe('NotGranted')
    expect(result.permissions.screen_capture).toBe(false)
  })
})
