import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { type RecoveryResult, useContextRecovery } from './useContextRecovery'

/**
 * `useContextRecovery` unit test (#8892).
 *
 * This hook is the single authoritative entry point into a current-scene
 * generate turn: it must invoke only `request_current_context_suggestions`, and
 * never substitute a read-only scene analysis or open an existing queue. The
 * return is metadata only (no raw text), and an in-flight guard blocks duplicate turns.
 */

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

const GENERATED: RecoveryResult = {
  status: 'generated',
  reason: null,
  admitted_count: 1,
  queue_count: 1,
  admitted_suggestion_ids: ['sug_1'],
  missing_permissions: [],
  provenance: { source_id: 'scene_1', observed_at: 'x', captured_at: 'y' },
}

describe('useContextRecovery', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  it('invokes request_current_context_suggestions and nothing else', async () => {
    mockInvoke.mockResolvedValue(GENERATED)
    const { result } = renderHook(() => useContextRecovery())

    await act(async () => {
      await result.current.generate('sess_1')
    })

    expect(result.current.phase).toBe('done')
    expect(result.current.result?.status).toBe('generated')
    const cmds = mockInvoke.mock.calls.map((c) => c[0])
    expect(cmds).toContain('request_current_context_suggestions')
    // Does not substitute a read-only scene analysis.
    expect(cmds).not.toContain('analyze_current_scene')
    // Passes sessionId in camelCase (Tauri v2 auto-converts to snake_case).
    expect(mockInvoke).toHaveBeenCalledWith('request_current_context_suggestions', {
      sessionId: 'sess_1',
    })
  })

  it('surfaces a typed non-generated outcome without throwing', async () => {
    mockInvoke.mockResolvedValue({ ...GENERATED, status: 'consent_required', missing_permissions: ['screen'] })
    const { result } = renderHook(() => useContextRecovery())

    await act(async () => {
      await result.current.generate()
    })

    expect(result.current.phase).toBe('done')
    expect(result.current.result?.status).toBe('consent_required')
    expect(result.current.result?.missing_permissions).toEqual(['screen'])
  })

  it('sets error phase when the backend rejects', async () => {
    mockInvoke.mockRejectedValue(new Error('provider offline'))
    const { result } = renderHook(() => useContextRecovery())

    await act(async () => {
      await result.current.generate()
    })

    expect(result.current.phase).toBe('error')
    expect(result.current.error).toContain('provider offline')
  })

  it('the in-flight guard blocks a concurrent second turn', async () => {
    // Leave the first turn unresolved, and call a second generate in the meantime.
    let release: (v: RecoveryResult) => void = () => {}
    const pending = new Promise<RecoveryResult>((res) => {
      release = res
    })
    mockInvoke.mockReturnValue(pending)
    const { result } = renderHook(() => useContextRecovery())

    // Call twice in the same synchronous frame: after the first call sets
    // inFlight, the second must hit that guard and return null immediately.
    let firstPromise!: Promise<RecoveryResult | null>
    let secondPromise!: Promise<RecoveryResult | null>
    act(() => {
      firstPromise = result.current.generate()
      secondPromise = result.current.generate()
    })
    await expect(secondPromise).resolves.toBeNull()

    // Complete the first turn.
    await act(async () => {
      release(GENERATED)
      await firstPromise
    })

    // Only the first turn dispatched a real command (the second was blocked by the guard and not sent).
    expect(mockInvoke.mock.calls.filter((c) => c[0] === 'request_current_context_suggestions').length).toBe(1)
  })

  it('reset returns to idle', async () => {
    mockInvoke.mockResolvedValue(GENERATED)
    const { result } = renderHook(() => useContextRecovery())
    await act(async () => {
      await result.current.generate()
    })
    expect(result.current.phase).toBe('done')
    await act(async () => {
      result.current.reset()
    })
    expect(result.current.phase).toBe('idle')
    expect(result.current.result).toBeNull()
  })
})
