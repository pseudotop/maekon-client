/**
 * #12530 — Settings > AI & Automation HTTP Chat verification, 5-locale parity.
 *
 * Every result status the verification command returns has its own message.
 * A locale missing one renders the raw dotted key exactly when the user needs
 * to know why the check failed, so the key set is asserted everywhere.
 */

import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

type JsonRecord = Record<string, unknown>

const LOCALES: Record<string, JsonRecord> = { en, ko, ja, 'zh-CN': zhCN, es }

/** Keys `ChatHttpVerification.tsx` resolves under `settingsAutomation.chatVerification`. */
const REQUIRED = [
  'action',
  'description',
  'result.budgetExhausted',
  'result.consentRequired',
  'result.failed',
  'result.notConfigured',
  'result.providerError',
  'result.restartRequired',
  'result.timeout',
  'result.verified',
  'saveFirst',
  'title',
  'verifying',
]

function flatEntries(obj: JsonRecord, prefix = ''): Array<[string, unknown]> {
  return Object.entries(obj).flatMap(([key, value]) => {
    const path = prefix ? `${prefix}.${key}` : key
    return value !== null && typeof value === 'object' && !Array.isArray(value)
      ? flatEntries(value as JsonRecord, path)
      : [[path, value] as [string, unknown]]
  })
}

function verificationBlock(locale: JsonRecord): JsonRecord {
  const settingsAutomation = locale.settingsAutomation as JsonRecord | undefined
  const block = settingsAutomation?.chatVerification as JsonRecord | undefined
  if (!block) throw new Error('missing settingsAutomation.chatVerification')
  return block
}

describe('HTTP Chat verification localization (#12530)', () => {
  it('keeps the same non-empty keys in every supported locale', () => {
    for (const [name, locale] of Object.entries(LOCALES)) {
      const entries = flatEntries(verificationBlock(locale))
      expect(entries.map(([key]) => key).sort(), name).toEqual(REQUIRED)
      for (const [key, value] of entries) {
        expect(typeof value === 'string' && value.trim() !== '', `${name}: ${key}`).toBe(true)
      }
    }
  })

  it('translates every message instead of falling back to English', () => {
    const english = new Map(flatEntries(verificationBlock(en)))
    for (const [name, locale] of Object.entries(LOCALES)) {
      if (name === 'en') continue
      for (const [key, value] of flatEntries(verificationBlock(locale))) {
        expect(value, `${name}: ${key}`).not.toBe(english.get(key))
      }
    }
  })
})
