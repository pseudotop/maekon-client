import { clearMocks, mockIPC, mockWindows } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it } from 'vitest'

describe('CRT-MK-M021: Window API contract', () => {
  afterEach(() => clearMocks())

  it('M021: mockWindows provides current window', async () => {
    mockWindows('main', 'settings')
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    const win = getCurrentWindow()
    expect(win.label).toBe('main')
  })

  it('M022: multiple windows accessible', async () => {
    mockWindows('main', 'settings', 'about')
    mockIPC((cmd) => {
      if (cmd === 'plugin:window|get_all_windows') {
        return ['main', 'settings', 'about']
      }
    })
    const { getAllWindows } = await import('@tauri-apps/api/window')
    const windows = await getAllWindows()
    expect(windows.length).toBeGreaterThanOrEqual(3)
    expect(windows.map((window) => window.label)).toEqual(['main', 'settings', 'about'])
  })
})
