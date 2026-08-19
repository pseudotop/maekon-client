/**
 * #9707 — OS handoff IPC wrapper.
 *
 * The allowlist itself is Rust-side and tested there
 * (`src-tauri/src/commands/os_handoff.rs`); duplicating it here would create a
 * second copy that can disagree with the enforcing one. What this suite covers
 * is the part only the TypeScript side can get wrong: the command name and
 * argument shape Tauri v2 expects, the one-call-one-handoff property, and the
 * failure classification callers branch on.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { HANDOFF_ERROR_CODES, HandoffBridgeUnavailableError, isHandoffError, openExternalTarget } from '../osHandoff'

const mockInvoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

describe('openExternalTarget (#9707)', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockInvoke.mockResolvedValue(undefined)
  })

  it('invokes the registered command with the camelCase argument Tauri v2 expects', async () => {
    // The Rust parameter is `url: String`; Tauri v2 matches invoke args by
    // name, so a renamed key here fails at runtime with nothing failing at
    // build time.
    await openExternalTarget('https://console.example.com/board')

    expect(mockInvoke).toHaveBeenCalledTimes(1)
    expect(mockInvoke).toHaveBeenCalledWith('open_external_target', {
      url: 'https://console.example.com/board',
    })
  })

  it('hands off exactly once per call and never retries', async () => {
    // A retry loop around an OS handoff turns one user gesture into several
    // compose windows.
    mockInvoke.mockRejectedValueOnce({
      code: HANDOFF_ERROR_CODES.launchFailed,
      message: 'xdg-open exited with 4',
    })

    await expect(openExternalTarget('mailto:a@example.com')).rejects.toMatchObject({
      code: HANDOFF_ERROR_CODES.launchFailed,
    })
    expect(mockInvoke).toHaveBeenCalledTimes(1)
  })

  it('passes the target through verbatim rather than pre-validating it', async () => {
    // Deliberate: the allowlist lives in Rust so a caller cannot route around
    // it. If this file started filtering, the enforcing copy and the convenient
    // copy would drift, and only one of them is the one that matters.
    await openExternalTarget('file:///etc/passwd')
    expect(mockInvoke).toHaveBeenCalledWith('open_external_target', { url: 'file:///etc/passwd' })
  })

  it('surfaces the Rust rejection envelope unchanged', async () => {
    const envelope = {
      code: HANDOFF_ERROR_CODES.rejected,
      message: 'scheme `file` is not handed off; only https and mailto are',
    }
    mockInvoke.mockRejectedValueOnce(envelope)

    await expect(openExternalTarget('file:///etc/passwd')).rejects.toEqual(envelope)
  })
})

describe('isHandoffError (#9707)', () => {
  it('recognises each handoff code, and narrows to one when asked', () => {
    for (const code of Object.values(HANDOFF_ERROR_CODES)) {
      expect(isHandoffError({ code, message: 'x' })).toBe(true)
    }

    // #9627 branches on "no mail app" specifically, so the narrowing form has
    // to actually discriminate.
    const noHandler = { code: HANDOFF_ERROR_CODES.noHandler, message: 'x' }
    expect(isHandoffError(noHandler, HANDOFF_ERROR_CODES.noHandler)).toBe(true)
    expect(isHandoffError(noHandler, HANDOFF_ERROR_CODES.launchFailed)).toBe(false)
  })

  it('does not claim unrelated rejections', () => {
    expect(isHandoffError({ code: 'auth.failed', message: 'x' })).toBe(false)
    expect(isHandoffError(new Error('boom'))).toBe(false)
    expect(isHandoffError('legacy string error')).toBe(false)
    expect(isHandoffError(null)).toBe(false)
  })

  it('does not classify a missing bridge as a handoff failure', () => {
    // Nothing was refused and nothing failed — there is no OS to hand to.
    // Folding it into the wire codes would make surfaces render "could not
    // open" where the honest answer is "not available here".
    expect(isHandoffError(new HandoffBridgeUnavailableError())).toBe(false)
  })
})
