import { invoke } from '@tauri-apps/api/core'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it } from 'vitest'

describe('CRT-MK-M013: get_web_port IPC contract', () => {
  afterEach(() => clearMocks())

  it('M013: returns port number', async () => {
    mockIPC((cmd) => {
      if (cmd === 'get_web_port') return 10090
    })
    const result = await invoke<number>('get_web_port')
    expect(typeof result).toBe('number')
    expect(result).toBe(10090)
  })

  it('M014: port is in expected range', async () => {
    mockIPC((cmd) => {
      if (cmd === 'get_web_port') return 10095
    })
    const result = await invoke<number>('get_web_port')
    expect(result).toBeGreaterThanOrEqual(10090)
    expect(result).toBeLessThanOrEqual(10099)
  })
})
