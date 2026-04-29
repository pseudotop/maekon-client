import { invoke } from '@tauri-apps/api/core'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it } from 'vitest'

describe('CRT-MK-M050: get_secret_backend_capabilities IPC contract', () => {
  afterEach(() => clearMocks())

  it('M050: returns secret backend capability snapshot', async () => {
    mockIPC((cmd) => {
      if (cmd === 'get_secret_backend_capabilities') {
        return {
          os_secret_store_available: true,
          oauth_available: true,
          oauth_provider_ids: ['openai', 'google'],
          default_backend_kind: 'os_secret_store',
          byok_backend_kind: 'os_secret_store',
          fallback_backend_kind: 'unavailable',
        }
      }
    })

    const result = await invoke<{
      os_secret_store_available: boolean
      oauth_available: boolean
      oauth_provider_ids: string[]
      default_backend_kind: string
      byok_backend_kind: string
      fallback_backend_kind: string
    }>('get_secret_backend_capabilities')

    expect(result.os_secret_store_available).toBe(true)
    expect(result.oauth_available).toBe(true)
    expect(result.oauth_provider_ids).toEqual(['openai', 'google'])
    expect(result.default_backend_kind).toBe('os_secret_store')
    expect(result.byok_backend_kind).toBe('os_secret_store')
    expect(result.fallback_backend_kind).toBe('unavailable')
  })
})
