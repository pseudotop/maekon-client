import { act, renderHook } from '@testing-library/react'
import type { ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { AppMemoryRouter } from '../../router/future'
import { NARROW_SHELL_QUERY, useShellLayout } from '../useShellLayout'

function wrapper({ children }: { children: ReactNode }) {
  return <AppMemoryRouter>{children}</AppMemoryRouter>
}

describe('useShellLayout', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('returns expected shape', () => {
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    expect(result.current).toHaveProperty('sidebarWidth')
    expect(result.current).toHaveProperty('sidebarCollapsed')
    expect(result.current).toHaveProperty('toggleSidebar')
    expect(result.current).toHaveProperty('onResizeStart')
    expect(result.current).toHaveProperty('onResizeByKeyboard')
  })

  it('default width is 260', () => {
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    expect(result.current.sidebarWidth).toBe(260)
  })

  it('default collapsed is false', () => {
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    expect(result.current.sidebarCollapsed).toBe(false)
  })

  it('toggleSidebar flips collapsed state', () => {
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    act(() => result.current.toggleSidebar())
    expect(result.current.sidebarCollapsed).toBe(true)
    act(() => result.current.toggleSidebar())
    expect(result.current.sidebarCollapsed).toBe(false)
  })

  it('onResizeByKeyboard changes width', () => {
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    const initialWidth = result.current.sidebarWidth
    act(() => result.current.onResizeByKeyboard(20))
    expect(result.current.sidebarWidth).toBe(initialWidth + 20)
  })

  it('width is clamped to min/max', () => {
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    // Try to go below min (200)
    act(() => result.current.onResizeByKeyboard(-500))
    expect(result.current.sidebarWidth).toBe(200)
    // Try to go above max (400)
    act(() => result.current.onResizeByKeyboard(500))
    expect(result.current.sidebarWidth).toBe(400)
  })
})

// #13736: windows at or below 900px start with the side panel collapsed.
describe('useShellLayout in a narrow window', () => {
  const originalMatchMedia = window.matchMedia
  let narrow = false
  let listeners: Array<(event: MediaQueryListEvent) => void> = []

  function resizeTo(width: number) {
    narrow = width <= 900
    act(() => {
      for (const listener of listeners) listener({ matches: narrow, media: NARROW_SHELL_QUERY } as MediaQueryListEvent)
    })
  }

  beforeEach(() => {
    localStorage.clear()
    narrow = false
    listeners = []
    window.matchMedia = vi.fn().mockImplementation((query: string) => ({
      get matches() {
        return query === NARROW_SHELL_QUERY ? narrow : false
      },
      media: query,
      onchange: null,
      addEventListener: (_type: string, listener: (event: MediaQueryListEvent) => void) => listeners.push(listener),
      removeEventListener: (_type: string, listener: (event: MediaQueryListEvent) => void) => {
        listeners = listeners.filter((l) => l !== listener)
      },
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })) as unknown as typeof window.matchMedia
  })

  afterEach(() => {
    window.matchMedia = originalMatchMedia
  })

  it('queries the 900px breakpoint', () => {
    expect(NARROW_SHELL_QUERY).toBe('(max-width: 900px)')
  })

  it('starts collapsed at 900px even when the saved choice is expanded', () => {
    localStorage.setItem('maekon-sidebar-collapsed', 'false')
    narrow = true
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    expect(result.current.sidebarCollapsed).toBe(true)
  })

  it('starts expanded at 901px', () => {
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    expect(result.current.sidebarCollapsed).toBe(false)
  })

  it('lets the user open it again without overwriting the saved choice', () => {
    localStorage.setItem('maekon-sidebar-collapsed', 'true')
    narrow = true
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    act(() => result.current.toggleSidebar())
    expect(result.current.sidebarCollapsed).toBe(false)
    expect(localStorage.getItem('maekon-sidebar-collapsed')).toBe('true')
  })

  it('collapses when the window shrinks and restores the saved choice when it grows', () => {
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    expect(result.current.sidebarCollapsed).toBe(false)

    resizeTo(820)
    expect(result.current.sidebarCollapsed).toBe(true)
    expect(localStorage.getItem('maekon-sidebar-collapsed')).toBe('false')

    resizeTo(1280)
    expect(result.current.sidebarCollapsed).toBe(false)
  })

  it('collapses again on the next shrink after the user had opened it', () => {
    narrow = true
    const { result } = renderHook(() => useShellLayout(), { wrapper })
    act(() => result.current.toggleSidebar())
    expect(result.current.sidebarCollapsed).toBe(false)

    resizeTo(1280)
    resizeTo(860)
    expect(result.current.sidebarCollapsed).toBe(true)
  })
})
