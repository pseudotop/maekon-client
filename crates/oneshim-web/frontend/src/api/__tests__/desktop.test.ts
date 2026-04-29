import { describe, expect, it } from 'vitest'

import { errorMessageFromInvoke, type IpcError, isIpcError } from '../desktop'

describe('isIpcError type guard', () => {
  it('matches a well-formed IpcError envelope', () => {
    const err: IpcError = { code: 'config.invalid', message: 'bad value' }
    expect(isIpcError(err)).toBe(true)
  })

  it('rejects a plain string (pre-migration command error)', () => {
    expect(isIpcError('legacy error string')).toBe(false)
  })

  it('rejects null', () => {
    expect(isIpcError(null)).toBe(false)
  })

  it('rejects an object missing the code field', () => {
    expect(isIpcError({ message: 'hi' })).toBe(false)
  })

  it('rejects an object missing the message field', () => {
    expect(isIpcError({ code: 'x.y' })).toBe(false)
  })

  it('rejects an object where code is not a string', () => {
    expect(isIpcError({ code: 42, message: 'hi' })).toBe(false)
  })

  it('rejects an Error instance', () => {
    // Errors are thrown by the JS runtime, not by Tauri commands —
    // they follow the Error.prototype shape, not IpcError.
    expect(isIpcError(new Error('boom'))).toBe(false)
  })

  it('accepts extra fields (forward-compat)', () => {
    // The serialization-shape test in Rust guards against adding new
    // fields, but if the contract grows intentionally the TS guard
    // should still validate the core shape.
    const err = { code: 'network.timeout', message: 't/o', extra: 'new' }
    expect(isIpcError(err)).toBe(true)
  })
})

describe('errorMessageFromInvoke', () => {
  it('unwraps an IpcError.message', () => {
    const err: IpcError = { code: 'config.invalid', message: 'bad value' }
    expect(errorMessageFromInvoke(err)).toBe('bad value')
  })

  it('falls back to a plain string (pre-migration command)', () => {
    expect(errorMessageFromInvoke('legacy string')).toBe('legacy string')
  })

  it('falls back to Error.message', () => {
    expect(errorMessageFromInvoke(new Error('exception'))).toBe('exception')
  })

  it('falls back to String() for an unknown shape', () => {
    expect(errorMessageFromInvoke(42)).toBe('42')
    expect(errorMessageFromInvoke(null)).toBe('null')
    expect(errorMessageFromInvoke(undefined)).toBe('undefined')
  })

  it('never returns an empty string', () => {
    // Defensive: even pathological inputs should produce something.
    const weird = { foo: 'bar' }
    expect(errorMessageFromInvoke(weird).length).toBeGreaterThan(0)
  })
})
