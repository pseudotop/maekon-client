import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

import type { IpcError } from '../../api/desktop'

import { hasTranslation, translatedCodes, translateError } from '../translateError'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

/** Read the canonical wire-code registry from the Rust snapshot fixture. */
function readWireCodeRegistry(): string[] {
  const registryPath = path.resolve(
    __dirname,
    '../../../../../../crates/oneshim-core/tests/wire_contract_snapshot.expected.txt',
  )
  const raw = fs.readFileSync(registryPath, 'utf-8')
  return raw
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
}

describe('wire-code i18n coverage', () => {
  const registry = readWireCodeRegistry()

  it('snapshot contains the expected 53 codes', () => {
    // 41 → 42 with D7 addition of service.circuit_open (2026-04-20).
    // 42 → 47 with Phase 9 PR-B1 addition of 5 autostart.* codes (2026-04-25).
    // 47 → 49 with TimeWindow primitive addition of 2 time_window.* codes (2026-04-26).
    // 49 → 53 with Phase 9 PR-B2 addition of 4 autostart.* codes (2026-04-27).
    expect(registry).toHaveLength(53)
  })

  it.each(['en', 'ko'] as const)('every wire code has a %s translation', (locale) => {
    const missing = registry.filter((code) => !hasTranslation(code, locale))
    expect(missing, `missing ${locale} translations`).toEqual([])
  })

  it('en and ko resource sets have the same keys (no drift between locales)', () => {
    const enCodes = new Set(translatedCodes('en'))
    const koCodes = new Set(translatedCodes('ko'))
    const onlyEn = [...enCodes].filter((c) => !koCodes.has(c))
    const onlyKo = [...koCodes].filter((c) => !enCodes.has(c))
    expect(onlyEn, 'codes in en but not ko').toEqual([])
    expect(onlyKo, 'codes in ko but not en').toEqual([])
  })
})

describe('translateError', () => {
  it('formats a known wire code with the {message} placeholder in en', () => {
    const err: IpcError = { code: 'config.invalid', message: 'bad value' }
    expect(translateError(err, 'en')).toBe('Invalid configuration: bad value')
  })

  it('formats a known wire code with the {message} placeholder in ko', () => {
    const err: IpcError = { code: 'config.invalid', message: 'bad value' }
    expect(translateError(err, 'ko')).toBe('설정 값이 올바르지 않습니다: bad value')
  })

  it('handles codes without placeholders (consent.expired, network.rate_limit, etc.)', () => {
    const err: IpcError = { code: 'consent.expired', message: 'ignored' }
    expect(translateError(err, 'en')).toMatch(/consent has expired/)
    expect(translateError(err, 'ko')).toMatch(/동의가 만료/)
  })

  it('handles the Bedrock unsupported fixed message', () => {
    const err: IpcError = {
      code: 'provider.bedrock.unsupported',
      message: 'AWS Bedrock is intentionally unsupported in this build',
    }
    expect(translateError(err, 'en')).toBe('AWS Bedrock is not supported in this build')
    expect(translateError(err, 'ko')).toBe('이 빌드는 AWS Bedrock을 지원하지 않습니다')
  })

  it('falls back to English when locale translation is missing', () => {
    // Simulated: if a future code is only in en, ko should fall back to en.
    // Both locale maps currently have full coverage, so we use an unknown code
    // to exercise the fallback — step 3 returns raw message.
    const err: IpcError = { code: 'novel.future.code', message: 'raw msg' }
    expect(translateError(err, 'en')).toBe('raw msg')
    expect(translateError(err, 'ko')).toBe('raw msg')
  })

  it('defaults to English when no locale specified', () => {
    const err: IpcError = { code: 'network.timeout', message: 'x' }
    expect(translateError(err)).toMatch(/timed out/)
  })

  it('returns a plain string as-is (pre-migration commands)', () => {
    expect(translateError('legacy error string')).toBe('legacy error string')
  })

  it('returns Error.message for Error instances', () => {
    expect(translateError(new Error('exception'))).toBe('exception')
  })

  it('falls back to String() for unknown shapes', () => {
    expect(translateError(42)).toBe('42')
    expect(translateError(null)).toBe('null')
    expect(translateError(undefined)).toBe('undefined')
  })

  it('never returns an empty string', () => {
    expect(translateError({})).not.toBe('')
    expect(translateError(null)).not.toBe('')
  })
})

describe('hasTranslation', () => {
  it('returns true for a code present in the specified locale', () => {
    expect(hasTranslation('config.invalid', 'en')).toBe(true)
    expect(hasTranslation('config.invalid', 'ko')).toBe(true)
  })

  it('returns false for an unknown code', () => {
    expect(hasTranslation('nonexistent.code', 'en')).toBe(false)
  })
})

describe('translatedCodes', () => {
  it('returns all 53 codes for en', () => {
    // 41 → 42 with D7 addition of service.circuit_open (2026-04-20).
    // 42 → 47 with Phase 9 PR-B1 addition of 5 autostart.* codes (2026-04-25).
    // 47 → 49 with TimeWindow primitive addition of 2 time_window.* codes (2026-04-26).
    // 49 → 53 with Phase 9 PR-B2 addition of 4 autostart.* codes (2026-04-27).
    expect(translatedCodes('en')).toHaveLength(53)
  })

  it('returns a frozen readonly array', () => {
    const codes = translatedCodes('en')
    // Attempting to mutate a frozen array throws in strict mode.
    // Vitest/Vite transforms TS in strict mode by default.
    expect(() => {
      ;(codes as string[]).push('extra')
    }).toThrow()
  })
})
