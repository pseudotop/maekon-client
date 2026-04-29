import { act, renderHook } from '@testing-library/react'
import type { ReactNode } from 'react'
import { beforeEach, describe, expect, it } from 'vitest'
import { AppMemoryRouter } from '../../router/future'
import { useShellLayout } from '../useShellLayout'

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
