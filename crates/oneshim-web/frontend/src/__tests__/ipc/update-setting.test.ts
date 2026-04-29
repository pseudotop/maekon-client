import { invoke } from '@tauri-apps/api/core'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it } from 'vitest'

describe('CRT-MK-M007: update_setting IPC contract', () => {
  afterEach(() => clearMocks())

  it('M007: accepts valid config_json with allowed key', async () => {
    mockIPC((cmd, args) => {
      if (cmd === 'update_setting') {
        const json = (args as any).config_json
        const parsed = JSON.parse(json)
        if (parsed.notification) return { ok: true }
        throw new Error('not permitted')
      }
    })
    const result = await invoke('update_setting', {
      config_json: JSON.stringify({ notification: { enabled: false } }),
    })
    expect(result).toEqual({ ok: true })
  })

  it('M008: rejects disallowed key', async () => {
    // NOTE: Layer 2 mock limitation — this tests the mock, not the real Rust allowlist.
    // The authoritative allowlist lives in commands.rs (ALLOWED_KEYS constant).
    // Drift detection is handled by:
    //   - Rust #[cfg(test)] allowed_keys_matches_expected_set (Layer 1)
    //   - WDIO get_allowed_setting_keys contract test (Layer 4)
    mockIPC((cmd, args) => {
      if (cmd === 'update_setting') {
        const json = (args as any).config_json
        const parsed = JSON.parse(json)
        // Reject any key that looks like a sensitive section
        const REJECTED = ['server', 'ai_provider', 'tls', 'grpc', 'sandbox', 'file_access']
        for (const key of Object.keys(parsed)) {
          if (REJECTED.includes(key)) throw new Error(`modifying '${key}' is not permitted`)
        }
        return { ok: true }
      }
    })
    await expect(invoke('update_setting', { config_json: JSON.stringify({ server: { url: 'x' } }) })).rejects.toThrow(
      /not permitted/,
    )
  })

  it('M009: handles malformed JSON', async () => {
    mockIPC((cmd, args) => {
      if (cmd === 'update_setting') {
        try {
          JSON.parse((args as any).config_json)
        } catch {
          throw new Error('invalid JSON')
        }
        return { ok: true }
      }
    })
    await expect(invoke('update_setting', { config_json: 'not-json{' })).rejects.toThrow(/invalid JSON/)
  })
})
