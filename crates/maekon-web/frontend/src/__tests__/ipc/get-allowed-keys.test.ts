import { invoke } from '@tauri-apps/api/core'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it } from 'vitest'

describe('CRT-MK-M040: get_allowed_setting_keys IPC contract', () => {
  afterEach(() => clearMocks())

  it('M040: returns array of strings', async () => {
    mockIPC((cmd) => {
      if (cmd === 'get_allowed_setting_keys') {
        return [
          'monitoring',
          'capture',
          'notification',
          'web',
          'schedule',
          'telemetry',
          'privacy',
          'update',
          'language',
          'theme',
        ]
      }
    })
    const keys = await invoke<string[]>('get_allowed_setting_keys')
    expect(Array.isArray(keys)).toBe(true)
    expect(keys.length).toBe(10)
    expect(keys).toContain('monitoring')
    expect(keys).toContain('privacy')
  })

  it('M041: does not contain sensitive keys', async () => {
    // NOTE: Layer 2 mock limitation — this tests the mock, not the real Rust list.
    // The authoritative test is the Rust #[cfg(test)] allowed_keys_excludes_sensitive_sections.
    // Layer 4 WDIO test T140 validates the real IPC response.
    mockIPC((cmd) => {
      if (cmd === 'get_allowed_setting_keys') {
        return [
          'monitoring',
          'capture',
          'notification',
          'web',
          'schedule',
          'telemetry',
          'privacy',
          'update',
          'language',
          'theme',
        ]
      }
    })
    const keys = await invoke<string[]>('get_allowed_setting_keys')
    const forbidden = ['server', 'ai_provider', 'tls', 'grpc', 'sandbox', 'file_access']
    for (const f of forbidden) {
      expect(keys).not.toContain(f)
    }
  })
})
