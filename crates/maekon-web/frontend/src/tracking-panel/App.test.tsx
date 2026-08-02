import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import { App, clampPanelPosition } from './App'

const mockInvoke = vi.fn()
const mockEmit = vi.fn()
const mockListen = vi.fn()
const mockOuterPosition = vi.fn()
const mockSetPosition = vi.fn()
const mockSetSize = vi.fn()
const mockCurrentMonitor = vi.fn()
const mockSuggestionUnlisten = vi.fn()
let captureStateListener:
  | ((event: {
      payload: { paused: boolean; indicator_visible: boolean; consent_granted: boolean; permitted: boolean }
    }) => void)
  | undefined
let suggestionChangedListener: (() => void) | undefined

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: (...args: unknown[]) => mockEmit(...args),
  listen: (...args: unknown[]) => mockListen(...args),
}))

vi.mock('@tauri-apps/api/window', () => ({
  currentMonitor: () => mockCurrentMonitor(),
  getCurrentWindow: () => ({
    outerPosition: (...args: unknown[]) => mockOuterPosition(...args),
    scaleFactor: vi.fn().mockResolvedValue(1),
    setPosition: (...args: unknown[]) => mockSetPosition(...args),
    setSize: (...args: unknown[]) => mockSetSize(...args),
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
    mockListen.mockReset()
    mockOuterPosition.mockReset().mockResolvedValue({ x: 0, y: 0 })
    mockSetPosition.mockReset().mockResolvedValue(undefined)
    mockSetSize.mockReset().mockResolvedValue(undefined)
    mockCurrentMonitor.mockReset().mockResolvedValue({
      workArea: { position: { x: 0, y: 0 }, size: { width: 1920, height: 1040 } },
    })
    mockSuggestionUnlisten.mockReset()
    captureStateListener = undefined
    suggestionChangedListener = undefined
    mockEmit.mockResolvedValue(undefined)
    mockListen.mockImplementation((event: string, listener: (...args: unknown[]) => void) => {
      if (event === 'overlay:capture-state-changed') {
        captureStateListener = (payload) => listener(payload)
      }
      if (event === 'overlay:suggestions-changed') {
        suggestionChangedListener = () => listener({ payload: { count: 999 } })
        return Promise.resolve(mockSuggestionUnlisten)
      }
      return Promise.resolve(vi.fn())
    })
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status')
        return Promise.resolve({ paused: false, indicator_visible: true, consent_granted: true, permitted: true })
      if (cmd === 'get_connection_status') return Promise.resolve({ server: false, llm: false, cli: false })
      if (cmd === 'get_pending_suggestion_count') return Promise.resolve(0)
      if (cmd === 'get_panel_position') return Promise.resolve(null)
      if (cmd === 'trigger_manual_capture') return Promise.resolve(undefined)
      return Promise.resolve(undefined)
    })
  })

  it('clamps expanded bounds to the active monitor work area', () => {
    expect(
      clampPanelPosition(
        { x: 1900, y: -394 },
        { width: 320, height: 430 },
        { position: { x: 0, y: 0 }, size: { width: 1920, height: 1040 } },
      ),
    ).toEqual({ x: 1600, y: 0 })
  })

  it('keeps a top-edge panel visible and restores its compact position after collapse', async () => {
    renderWithProviders(<App />)

    await act(async () => {
      fireEvent.click(screen.getByTitle('Expand'))
    })
    await waitFor(() => expect(mockSetPosition).toHaveBeenCalled())
    expect(mockSetPosition.mock.calls[0]?.[0]).toMatchObject({ x: 0, y: 0 })
    expect(mockSetSize).toHaveBeenCalledWith(expect.objectContaining({ width: 320, height: 430 }))

    await act(async () => {
      fireEvent.click(screen.getByTitle('Collapse'))
    })
    await waitFor(() => expect(mockSetPosition).toHaveBeenCalledTimes(2))
    expect(mockSetPosition.mock.calls[1]?.[0]).toMatchObject({ x: 0, y: 0 })
    expect(mockSetSize).toHaveBeenLastCalledWith(expect.objectContaining({ width: 260, height: 36 }))
  })

  it('shows consent required and disables pause before screen consent', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') {
        return Promise.resolve({
          paused: false,
          indicator_visible: true,
          consent_granted: false,
          permitted: false,
        })
      }
      if (cmd === 'get_connection_status') return Promise.resolve({ server: false, llm: false, cli: false })
      if (cmd === 'get_panel_position') return Promise.resolve(null)
      return Promise.resolve(undefined)
    })

    renderWithProviders(<App />)

    expect((await screen.findAllByText('Consent required')).length).toBeGreaterThan(0)
    expect(screen.queryByText('Capturing')).not.toBeInTheDocument()
    expect(screen.getByTitle('Consent required')).toBeDisabled()
  })
  it('describes disconnected service lanes as local mode instead of whole-app offline', async () => {
    renderWithProviders(<App />)

    await act(async () => {
      fireEvent.click(screen.getByTitle('Expand'))
    })

    expect((await screen.findAllByText(/local mode/i)).length).toBeGreaterThan(0)
    expect(screen.queryByText(/^Offline/i)).not.toBeInTheDocument()
  })

  it('repaints compact and expanded labels when capture becomes paused', async () => {
    renderWithProviders(<App />)

    expect(await screen.findByText('Capturing')).toHaveAttribute('data-capture-state', 'capturing')
    await waitFor(() => expect(captureStateListener).toBeDefined())

    await act(async () => {
      captureStateListener?.({
        payload: { paused: true, indicator_visible: true, consent_granted: true, permitted: false },
      })
    })

    expect(screen.getByText('Paused')).toHaveAttribute('data-capture-state', 'paused')
    expect(screen.getByTitle('Resume')).toHaveTextContent('▶')

    await act(async () => {
      fireEvent.click(screen.getByTitle('Expand'))
    })
    expect(screen.getByText('Screen context paused')).toBeInTheDocument()

    await act(async () => {
      captureStateListener?.({
        payload: { paused: false, indicator_visible: true, consent_granted: true, permitted: true },
      })
    })

    expect(screen.getByText('Capturing')).toHaveAttribute('data-capture-state', 'capturing')
    expect(screen.getByTitle('Pause')).toHaveTextContent('⏸')
    expect(screen.getByText('Screen context ready')).toBeInTheDocument()
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

  it('hydrates and refreshes the authoritative pending count independently from scene analysis', async () => {
    let pendingCount = 2
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status')
        return Promise.resolve({ paused: false, indicator_visible: true, consent_granted: true, permitted: true })
      if (cmd === 'get_connection_status') return Promise.resolve({ server: false, llm: false, cli: false })
      if (cmd === 'get_pending_suggestion_count') return Promise.resolve(pendingCount)
      if (cmd === 'get_panel_position') return Promise.resolve(null)
      if (cmd === 'analyze_current_scene') {
        return Promise.resolve({
          app_name: 'Editor',
          window_title: 'Issue 8572',
          accessibility: { element_count: 4 },
          ocr_regions: [],
          gui_elements: [],
        })
      }
      return Promise.resolve(undefined)
    })

    const { container } = renderWithProviders(<App />)
    const badge = () => container.querySelector('[data-visual-region="collapsed-suggestion-count"]')

    await waitFor(() => expect(badge()).toHaveTextContent('2'))
    await act(async () => {
      fireEvent.click(screen.getByTitle('Expand'))
    })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Review current window' }))
    })
    await screen.findByText(/Editor — Issue 8572/)
    expect(badge()).toHaveTextContent('2')

    pendingCount = 1
    await act(async () => {
      suggestionChangedListener?.()
    })
    await waitFor(() => expect(badge()).toHaveTextContent('1'))
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'get_pending_suggestion_count')).toHaveLength(2)
  })

  it('fails closed to zero and removes the queue listener on unmount', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status')
        return Promise.resolve({ paused: false, indicator_visible: true, consent_granted: true, permitted: true })
      if (cmd === 'get_connection_status') return Promise.resolve({ server: false, llm: false, cli: false })
      if (cmd === 'get_pending_suggestion_count') return Promise.reject(new Error('count unavailable'))
      if (cmd === 'get_panel_position') return Promise.resolve(null)
      return Promise.resolve(undefined)
    })

    const { container, unmount } = renderWithProviders(<App />)
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('get_pending_suggestion_count', undefined))
    expect(container.querySelector('[data-visual-region="collapsed-suggestion-count"]')).toHaveTextContent('0')
    await waitFor(() => expect(suggestionChangedListener).toBeDefined())

    unmount()
    expect(mockSuggestionUnlisten).toHaveBeenCalledOnce()
  })

  it('renders a sectioned floating command menu with running, pinned, recent, usage, and app actions', async () => {
    renderWithProviders(<App />)

    await act(async () => {
      fireEvent.click(screen.getByTitle('Expand'))
    })

    const menu = await screen.findByRole('region', { name: 'Floating command menu' })
    expect(menu).toHaveClass('max-h-[calc(100vh-2.5rem)]', 'overflow-y-auto')
    expect(menu).toHaveTextContent('Running')
    expect(menu).toHaveTextContent('Pinned')
    expect(menu).toHaveTextContent('Recent')
    expect(menu).toHaveTextContent('Usage')
    expect(menu).toHaveTextContent('Screen context ready')
    expect(menu).toHaveTextContent('0 captures this session')
    expect(menu).toHaveTextContent('0/3 service lanes connected')
    expect(screen.getByRole('button', { name: 'Find my next step' })).toBeEnabled()
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
      expect(mockInvoke).toHaveBeenCalledWith('request_app_quit', undefined)
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
