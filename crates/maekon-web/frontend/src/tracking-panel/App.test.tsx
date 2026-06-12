import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import { App } from './App'

const mockInvoke = vi.fn()
const mockEmit = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: (...args: unknown[]) => mockEmit(...args),
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
    mockEmit.mockReset()
    mockEmit.mockResolvedValue(undefined)
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

    expect((await screen.findAllByText(/local mode/i)).length).toBeGreaterThan(0)
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

  it('opens the GUI suggestion panel idempotently and requests native panel mode', async () => {
    renderWithProviders(<App />)

    await act(async () => {
      fireEvent.click(screen.getByTitle('Expand'))
    })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'AI Suggestions' }))
    })

    await waitFor(() => {
      expect(mockEmit).toHaveBeenCalledWith('overlay:set-suggestions-panel', { open: true })
    })
    expect(mockInvoke).toHaveBeenCalledWith('toggle_suggestions_panel', { open: true })
  })

  it('renders a sectioned floating command menu with running, pinned, recent, usage, and app actions', async () => {
    renderWithProviders(<App />)

    await act(async () => {
      fireEvent.click(screen.getByTitle('Expand'))
    })

    const menu = await screen.findByRole('region', { name: 'Floating command menu' })
    expect(menu).toHaveTextContent('Running')
    expect(menu).toHaveTextContent('Pinned')
    expect(menu).toHaveTextContent('Recent')
    expect(menu).toHaveTextContent('Usage')
    expect(menu).toHaveTextContent('Screen context ready')
    expect(menu).toHaveTextContent('0 captures this session')
    expect(menu).toHaveTextContent('0/3 service lanes connected')
    expect(screen.getByRole('button', { name: 'Extract tasks from current app' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Review current window' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'New Chat' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Open Maekon' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Quit Maekon' })).toBeEnabled()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'New Chat' }))
    })
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('show_main_window', undefined)
      expect(mockEmit).toHaveBeenCalledWith('navigate:chat', {})
    })

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Quit Maekon' }))
    })
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('simulate_tray_action', { action: 'quit' })
    })
  })

  it('exposes stable bottom floating bar visual oracle regions from the imagegen target contract', async () => {
    const { container } = renderWithProviders(<App />)
    const toolbar = screen.getByRole('toolbar')

    for (const region of ['floating-bar-anchor', 'screen-context-status', 'collapsed-suggestion-count']) {
      expect(container.querySelector(`[data-visual-region="${region}"]`)).toBeInTheDocument()
    }
    expect(toolbar.querySelector('[data-visual-region="collapsed-suggestion-count"]')).toBeInTheDocument()

    await act(async () => {
      fireEvent.click(screen.getByTitle('Expand'))
    })

    for (const region of [
      'floating-bar-anchor',
      'screen-context-status',
      'provider-health-dots',
      'quick-actions',
      'keyboard-a11y-action',
      'collapsed-suggestion-count',
    ]) {
      expect(container.querySelector(`[data-visual-region="${region}"]`)).toBeInTheDocument()
    }
    expect(container.querySelectorAll('[data-visual-region="collapsed-suggestion-count"]')).toHaveLength(1)
  })
})
