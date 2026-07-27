/**
 * #8044: capture-history re-authentication i18n 5-locale parity test.
 *
 * Verifies that the (nested-inclusive) key set of the `reauth.*` namespace
 * is identical across all 5 locales. If any locale omits a key or has a
 * typo, this fails. Also pins that every key used by CaptureReauthGate /
 * CaptureReauthSettings exists in en.
 */

import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const

type JsonRecord = Record<string, unknown>

function reauthBlock(locale: JsonRecord): JsonRecord {
  const reauth = locale.reauth as JsonRecord | undefined
  if (!reauth) {
    throw new Error('missing reauth block')
  }
  return reauth
}

function flatKeys(obj: JsonRecord, prefix = ''): string[] {
  const out: string[] = []
  for (const [k, v] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${k}` : k
    if (v !== null && typeof v === 'object' && !Array.isArray(v)) {
      out.push(...flatKeys(v as JsonRecord, path))
    } else {
      out.push(path)
    }
  }
  return out.sort()
}

describe('reauth i18n parity (5 locales)', () => {
  const reference = flatKeys(reauthBlock(en as JsonRecord))

  it('en baseline has the load-bearing keys', () => {
    // The core keys the components actually use must exist (regression guard against renames).
    const required = [
      'title',
      'description',
      'reason',
      'useBiometric',
      'usePinInstead',
      'unlock',
      'errors.cancelled',
      'errors.biometricUnavailable',
      'errors.failed',
      'settings.sectionTitle',
      'settings.enableLabel',
      'settings.idleLabel',
      'settings.registerPin',
      'settings.removePin',
      'settings.pinMismatch',
      'settings.saveFailed',
    ]
    for (const key of required) {
      expect(reference).toContain(key)
    }
  })

  for (const [name, locale] of Object.entries(LOCALES)) {
    it(`${name} has the same reauth key set as en`, () => {
      expect(flatKeys(reauthBlock(locale as JsonRecord))).toEqual(reference)
    })
  }
})
