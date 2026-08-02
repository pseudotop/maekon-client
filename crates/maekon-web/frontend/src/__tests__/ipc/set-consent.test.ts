import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { setConsent } from '../../api/client'
import type { ConsentPermissions } from '../../api/contracts'

// set_consent(permissions) → ConsentSnapshot. Tauri serializes invoke argument
// keys verbatim, so ConsentPermissions (snake_case) is passed under the `permissions` key.
describe('set_consent IPC contract', () => {
  afterEach(() => clearMocks())

  it('passes the permissions arg and returns the granted snapshot', async () => {
    const argSpy = vi.fn()

    mockIPC((cmd, args) => {
      if (cmd === 'set_consent') {
        argSpy(args)
        const perms = (args as { permissions: ConsentPermissions }).permissions
        return {
          status: 'Valid',
          permissions: perms,
        }
      }
    })

    const permissions: ConsentPermissions = {
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
      memory_graph_retrieval_ranking: false,
      memory_vault_mirror: false,
    }

    const result = await setConsent(permissions)

    expect(argSpy).toHaveBeenCalledWith({ permissions })
    expect(result.status).toBe('Valid')
    expect(result.permissions.screen_capture).toBe(true)
  })
})
