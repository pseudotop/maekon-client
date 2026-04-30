import { describe, expect, it } from 'vitest'
import { redact } from '../redact'

describe('redact', () => {
  it('passes through nullish inputs', () => {
    expect(redact(undefined)).toBeUndefined()
    expect(redact(null)).toBeUndefined()
  })

  it('returns plain strings unchanged', () => {
    expect(redact('hello world')).toBe('hello world')
  })

  it('masks macOS user home paths', () => {
    const input = 'Failed at /Users/alice/project/file.tsx:10:5'
    const output = redact(input)
    expect(output).not.toContain('/Users/alice')
    expect(output).toContain('~')
  })

  it('masks Linux user home paths', () => {
    const input = 'Error at /home/bob/code/app.ts'
    const output = redact(input)
    expect(output).not.toContain('/home/bob')
    expect(output).toContain('~')
  })

  it('masks Windows user paths', () => {
    const input = 'Stack: C:\\Users\\Charlie\\Documents\\app.exe'
    const output = redact(input)
    expect(output).not.toContain('C:\\Users\\Charlie')
  })

  it('masks Windows usernames containing the letter s (regex regression)', () => {
    // Earlier bug: the character class `[^\\s"]` excluded literal 's', not
    // whitespace, so "Sam", "Steven", etc. leaked. Verify the fix.
    for (const name of ['Sam', 'Steven', 'Tess', 'Sasha']) {
      const input = `Failed in C:\\Users\\${name}\\AppData\\app.log`
      const output = redact(input)
      expect(output).not.toContain(`C:\\Users\\${name}`)
    }
  })

  it('masks /var/folders temp paths', () => {
    const input = 'Cache miss /var/folders/xy/abc123/T/cache.dat'
    const output = redact(input)
    expect(output).not.toContain('/var/folders/xy/abc123')
  })

  it('masks OpenAI/Anthropic-style sk- API keys', () => {
    const input = 'Auth header: sk-abc123def456ghi789jkl012'
    const output = redact(input)
    expect(output).not.toContain('sk-abc123def456')
    expect(output).toContain('[REDACTED]')
  })

  it('masks Bearer tokens', () => {
    const input = 'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.token.signature'
    const output = redact(input)
    expect(output).not.toContain('Bearer eyJhbGciOiJIUzI1NiJ9')
    expect(output).toContain('[REDACTED]')
  })

  it('masks Bearer tokens containing the full base64url alphabet (NC-NEW-7 + U3)', () => {
    // Verify the regex character class still matches all valid base64url
    // characters: A-Z, a-z, 0-9, plus +, /, -, _, ~, .
    // After NC-NEW-7 fix the class is [A-Za-z0-9._~+/-]+, where `-` is
    // literal (last in class) instead of accidentally forming a range.
    const input = 'Authorization: Bearer abc+def/ghi-jkl_mno~pqr.stu=='
    const output = redact(input)
    expect(output).not.toContain('abc+def/ghi-jkl_mno~pqr.stu')
    expect(output).toContain('[REDACTED]')
  })

  it('masks JWT shape tokens standalone', () => {
    const input = 'token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c'
    const output = redact(input)
    expect(output).toContain('[REDACTED]')
  })

  it('masks api_key=value patterns', () => {
    const input = 'Config: api_key="abcdef123456"'
    const output = redact(input)
    expect(output).not.toContain('abcdef123456')
    expect(output).toContain('[REDACTED]')
  })

  it('preserves non-secret content around masked tokens', () => {
    const input = 'Error in fetch with sk-1234567890abcdefgh: 401 Unauthorized'
    const output = redact(input)
    expect(output).toContain('Error in fetch with')
    expect(output).toContain('401 Unauthorized')
    expect(output).not.toContain('sk-1234567890abcdefgh')
  })

  it('handles multiple secrets in one string', () => {
    const input = 'sk-aaaaaaaaaaaaaaaaaaaa and sk-bbbbbbbbbbbbbbbbbbbb'
    const output = redact(input)
    expect(output).not.toContain('sk-aaaaaaaa')
    expect(output).not.toContain('sk-bbbbbbbb')
    const matches = (output ?? '').match(/\[REDACTED\]/g)
    expect(matches?.length).toBe(2)
  })
})
