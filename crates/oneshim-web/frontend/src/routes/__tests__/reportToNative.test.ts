import { beforeEach, describe, expect, it, vi } from 'vitest'

// Mock @tauri-apps/api/core — dynamic import in reportToNative.ts resolves this
const invokeMock = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

// Mock platform module — simulate Tauri runtime
vi.mock('../../utils/platform', () => ({ IS_TAURI: true }))

// Mock frontend logger to avoid real IPC bridge
vi.mock('../../logging/frontendLogger', () => ({ logFrontend: vi.fn() }))

import { reportToNative } from '../reportToNative'

/**
 * Flush microtasks so the fire-and-forget IIFE inside reportToNative has a
 * chance to resolve the dynamic import and invoke the mock.
 */
async function flushMicrotasks() {
  // Multiple ticks: dynamic import resolution + async IIFE body + invoke await.
  await Promise.resolve()
  await Promise.resolve()
  await new Promise((resolve) => setTimeout(resolve, 0))
}

describe('reportToNative', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    invokeMock.mockResolvedValue(undefined)
  })

  it('sends errorMessage field (not message) to match Rust contract', async () => {
    reportToNative({
      route: '/focus',
      severity: 'error',
      message: 'Component crashed',
      stack: 'Error: ...',
    })

    await flushMicrotasks()

    expect(invokeMock).toHaveBeenCalledTimes(1)
    expect(invokeMock).toHaveBeenCalledWith(
      'report_frontend_error',
      expect.objectContaining({
        route: '/focus',
        severity: 'error',
        errorMessage: 'Component crashed', // CRITICAL: must be errorMessage, not message
        stack: 'Error: ...',
      }),
    )
  })

  it('does not send a `message` key — regression guard for IPC field name', async () => {
    reportToNative({
      route: '/focus',
      severity: 'error',
      message: 'Component crashed',
    })

    await flushMicrotasks()

    expect(invokeMock).toHaveBeenCalledTimes(1)
    const [, payload] = invokeMock.mock.calls[0]
    expect(payload).toHaveProperty('errorMessage')
    expect(payload).not.toHaveProperty('message')
  })

  it('passes null stack when stack is undefined', async () => {
    reportToNative({ route: '/test', severity: 'warning', message: 'msg' })

    await flushMicrotasks()

    expect(invokeMock).toHaveBeenCalledWith(
      'report_frontend_error',
      expect.objectContaining({
        stack: null,
      }),
    )
  })

  it('invokes the report_frontend_error command', async () => {
    reportToNative({
      route: '/dashboard',
      severity: 'critical',
      message: 'Escalated',
      stack: 'stack trace',
    })

    await flushMicrotasks()

    expect(invokeMock).toHaveBeenCalledWith('report_frontend_error', expect.any(Object))
  })

  it('forwards severity and route verbatim', async () => {
    reportToNative({
      route: '/automation/logs',
      severity: 'warning',
      message: 'Network failure',
      stack: 'stacktrace',
    })

    await flushMicrotasks()

    expect(invokeMock).toHaveBeenCalledWith(
      'report_frontend_error',
      expect.objectContaining({
        route: '/automation/logs',
        severity: 'warning',
      }),
    )
  })

  it('swallows invoke rejections (fire-and-forget)', async () => {
    invokeMock.mockRejectedValue(new Error('ipc error'))

    expect(() =>
      reportToNative({
        route: '/test',
        severity: 'error',
        message: 'boom',
      }),
    ).not.toThrow()

    await flushMicrotasks()
    // Rejection must not propagate — reportToNative is intentionally
    // fire-and-forget and the caller should not need a .catch.
    expect(invokeMock).toHaveBeenCalled()
  })

  it('forwards componentStack to the Rust command', async () => {
    reportToNative({
      route: '/focus',
      severity: 'error',
      message: 'Component crashed',
      componentStack: '\n    at FocusSection\n    at FocusLayout',
    })

    await flushMicrotasks()

    expect(invokeMock).toHaveBeenCalledWith(
      'report_frontend_error',
      expect.objectContaining({
        componentStack: expect.stringContaining('FocusSection'),
      }),
    )
  })

  it('redacts file paths in the error message', async () => {
    reportToNative({
      route: '/focus',
      severity: 'error',
      message: 'Failed at /Users/alice/project/src/file.tsx',
    })

    await flushMicrotasks()

    const [, payload] = invokeMock.mock.calls[0]
    expect(payload.errorMessage).not.toContain('/Users/alice')
  })

  it('redacts API keys in the stack', async () => {
    reportToNative({
      route: '/test',
      severity: 'error',
      message: 'auth failed',
      stack: 'Error\n  at fetch (sk-1234567890abcdefghij)',
    })

    await flushMicrotasks()

    const [, payload] = invokeMock.mock.calls[0]
    expect(payload.stack).not.toContain('sk-1234567890abcdefghij')
    expect(payload.stack).toContain('[REDACTED]')
  })
})
