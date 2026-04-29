import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import { App } from './App'

const mockInvoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(),
  listen: vi.fn().mockResolvedValue(vi.fn()),
}))

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    outerPosition: vi.fn().mockResolvedValue({ x: 0, y: 0 }),
    scaleFactor: vi.fn().mockResolvedValue(1),
    setPosition: vi.fn().mockResolvedValue(undefined),
    setSize: vi.fn().mockResolvedValue(undefined),
    startDragging: vi.fn(),
  }),
}))

vi.mock('@tauri-apps/api/dpi', () => ({
  LogicalPosition: class {
    constructor(
      readonly x: number,
      readonly y: number,
    ) {}
  },
  LogicalSize: class {
    constructor(
      readonly width: number,
      readonly height: number,
    ) {}
  },
}))

describe('tracking panel', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_connection_status') return Promise.resolve({ server: false, llm: false, cli: false })
      if (cmd === 'get_panel_position') return Promise.resolve(null)
      if (cmd === 'trigger_manual_capture') return Promise.resolve(undefined)
      return Promise.resolve(undefined)
    })
  })

  it('describes disconnected service lanes as local mode instead of whole-app offline', async () => {
    renderWithProviders(<App />)

    await act(async () => {
      fireEvent.click(screen.getByTitle('Expand'))
    })

    expect(await screen.findByText(/local mode/i)).toBeInTheDocument()
    expect(screen.queryByText(/^Offline/i)).not.toBeInTheDocument()
  })

  it('shows manual capture feedback inside the expanded panel status area', async () => {
    renderWithProviders(<App />)

    await act(async () => {
      fireEvent.click(screen.getByTitle('Expand'))
    })
    await act(async () => {
      fireEvent.click(screen.getByTitle('Manual Capture'))
    })

    await waitFor(() => {
      expect(screen.getByRole('status')).toHaveTextContent('Captured')
    })
  })
})
