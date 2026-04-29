import { afterEach, describe, expect, it, vi } from 'vitest'
import { _resetRecoverySignalsForTest, notifyRouteRecovery, subscribeToRouteRecovery } from '../recoverySignals'

describe('recoverySignals', () => {
  afterEach(() => {
    _resetRecoverySignalsForTest()
  })

  it('notify calls subscribed listener for matching route', () => {
    const listener = vi.fn()
    subscribeToRouteRecovery('/focus', listener)

    notifyRouteRecovery('/focus')

    expect(listener).toHaveBeenCalledTimes(1)
  })

  it('notify does not call listeners for other routes', () => {
    const focusListener = vi.fn()
    const dashListener = vi.fn()
    subscribeToRouteRecovery('/focus', focusListener)
    subscribeToRouteRecovery('/dashboard', dashListener)

    notifyRouteRecovery('/focus')

    expect(focusListener).toHaveBeenCalledTimes(1)
    expect(dashListener).not.toHaveBeenCalled()
  })

  it('notify is a no-op when no listeners exist', () => {
    expect(() => notifyRouteRecovery('/never-subscribed')).not.toThrow()
  })

  it('unsubscribe removes the listener', () => {
    const listener = vi.fn()
    const unsubscribe = subscribeToRouteRecovery('/test', listener)

    notifyRouteRecovery('/test')
    expect(listener).toHaveBeenCalledTimes(1)

    unsubscribe()
    notifyRouteRecovery('/test')
    expect(listener).toHaveBeenCalledTimes(1)
  })

  it('multiple listeners on the same route all fire', () => {
    const a = vi.fn()
    const b = vi.fn()
    subscribeToRouteRecovery('/shared', a)
    subscribeToRouteRecovery('/shared', b)

    notifyRouteRecovery('/shared')

    expect(a).toHaveBeenCalledTimes(1)
    expect(b).toHaveBeenCalledTimes(1)
  })

  it('listener that unsubscribes itself during notify does not break iteration', () => {
    const a = vi.fn()
    let unsubscribe: (() => void) | undefined
    const selfUnsub = vi.fn(() => {
      unsubscribe?.()
    })
    const c = vi.fn()
    subscribeToRouteRecovery('/iterating', a)
    unsubscribe = subscribeToRouteRecovery('/iterating', selfUnsub)
    subscribeToRouteRecovery('/iterating', c)

    notifyRouteRecovery('/iterating')

    expect(a).toHaveBeenCalledTimes(1)
    expect(selfUnsub).toHaveBeenCalledTimes(1)
    expect(c).toHaveBeenCalledTimes(1)
  })

  it('notify is not exposed via window globals', () => {
    expect((window as Record<string, unknown>).notifyRouteRecovery).toBeUndefined()
  })
})
