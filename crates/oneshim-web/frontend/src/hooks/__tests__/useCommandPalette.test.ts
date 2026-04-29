import { act, renderHook } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { useCommandPalette } from '../useCommandPalette'

describe('useCommandPalette', () => {
  it('returns expected shape', () => {
    const { result } = renderHook(() => useCommandPalette())
    expect(result.current).toHaveProperty('isOpen')
    expect(result.current).toHaveProperty('open')
    expect(result.current).toHaveProperty('close')
    expect(result.current).toHaveProperty('toggle')
  })

  it('initial state is closed', () => {
    const { result } = renderHook(() => useCommandPalette())
    expect(result.current.isOpen).toBe(false)
  })

  it('open() sets isOpen to true', () => {
    const { result } = renderHook(() => useCommandPalette())
    act(() => result.current.open())
    expect(result.current.isOpen).toBe(true)
  })

  it('close() sets isOpen to false', () => {
    const { result } = renderHook(() => useCommandPalette())
    act(() => result.current.open())
    act(() => result.current.close())
    expect(result.current.isOpen).toBe(false)
  })

  it('toggle() flips isOpen', () => {
    const { result } = renderHook(() => useCommandPalette())
    act(() => result.current.toggle())
    expect(result.current.isOpen).toBe(true)
    act(() => result.current.toggle())
    expect(result.current.isOpen).toBe(false)
  })
})
