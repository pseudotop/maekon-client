import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { getConsent } from '../../api/client'

// 와이어 계약 검증: maekon-core/src/consent.rs 의 ConsentPermissions(snake_case,
// rename_all 없음) + ConsentStatus(rename_all="PascalCase") 필드/문자열과
// src-tauri/src/commands/consent.rs 의 ConsentSnapshot DTO 를 그대로 미러링한다.
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
          },
        }
      }
    })

    const result = await getConsent()

    expect(invokeSpy).toHaveBeenCalledWith('get_consent')
    expect(result.status).toBe('Valid')
    expect(result.permissions.screen_capture).toBe(true)
    expect(result.permissions.memory_graph_enrichment).toBe(false)
  })
})
