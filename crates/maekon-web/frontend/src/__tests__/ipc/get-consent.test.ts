import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { getConsent } from '../../api/client'

// Wire contract verification: mirrors exactly the ConsentPermissions (snake_case,
// no rename_all) + ConsentStatus (rename_all="PascalCase") fields/strings from
// maekon-core/src/consent.rs and the ConsentSnapshot DTO from
// src-tauri/src/commands/consent.rs.
describe('get_consent IPC contract', () => {
  afterEach(() => clearMocks())

  it('returns the consent snapshot with the exact wire field keys', async () => {
    const invokeSpy = vi.fn()

    mockIPC((cmd) => {
      invokeSpy(cmd)
      if (cmd === 'get_consent') {
        return {
          status: 'Valid',
          permissions: {
            screen_capture: true,
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

    const result = await getConsent()

    expect(invokeSpy).toHaveBeenCalledWith('get_consent')
    expect(result.status).toBe('Valid')
    expect(result.permissions.screen_capture).toBe(true)
    expect(result.permissions.memory_graph_enrichment).toBe(false)
    expect(result.permissions.unredacted_external_ocr).toBe(false)
  })
})
