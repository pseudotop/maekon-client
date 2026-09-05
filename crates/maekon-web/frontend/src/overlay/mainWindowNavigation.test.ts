import { beforeEach, describe, expect, it, vi } from 'vitest'
import { navigateMainWindow } from './mainWindowNavigation'

const mocks = vi.hoisted(() => ({ emit: vi.fn(), invoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ emit: mocks.emit }))

describe('navigateMainWindow (#11735)', () => {
  beforeEach(() => {
    mocks.invoke.mockReset().mockResolvedValue(undefined)
    mocks.emit.mockReset().mockResolvedValue(undefined)
  })

  it('restores the main window before emitting the in-app route', async () => {
    const order: string[] = []
    mocks.invoke.mockImplementation(async () => order.push('show'))
    mocks.emit.mockImplementation(async () => order.push('navigate'))

    await navigateMainWindow('/settings/ai-automation')

    expect(mocks.invoke).toHaveBeenCalledWith('show_main_window')
    expect(mocks.emit).toHaveBeenCalledWith('navigate', '/settings/ai-automation')
    expect(order).toEqual(['show', 'navigate'])
  })
})
