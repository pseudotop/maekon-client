/**
 * Microphone-consent i18n 5-locale parity test (#4568 Phase B, design §4/§7).
 *
 * The ConsentToggleSection behavior tests (A.2 suite) use fallbackLng='en', so they
 * resolve through en only and cannot catch missing keys in non-en locales. This test
 * loads all 5 JSON files *directly* and asserts that the (nested-inclusive) key set of
 * `privacy.consent.microphone.*` is identical across all 5 locales.
 * If any locale omits a key (or has a typo), this test fails.
 */

import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const

type JsonRecord = Record<string, unknown>

/** Extracts the privacy.consent.microphone block. If absent, makes the test fail explicitly. */
function micBlock(locale: JsonRecord): JsonRecord {
  const privacy = locale.privacy as JsonRecord | undefined
  const consent = privacy?.consent as JsonRecord | undefined
  const microphone = consent?.microphone as JsonRecord | undefined
  if (!microphone) {
    throw new Error('privacy.consent.microphone 블록이 없습니다')
  }
  return microphone
}

/** Flattens a nested object into a sorted list of dot-separated path keys (leaf keys only). */
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

describe('privacy.consent.microphone i18n parity (5 locales)', () => {
  // Use en as the baseline key set.
  const reference = flatKeys(micBlock(en as JsonRecord))

  it('en defines the full expected microphone key-set', () => {
    // The exact leaf-key list required by the design (explicit assertion to prevent regressions).
    expect(reference).toEqual([
      'collected.audioCapture',
      'collected.cloudEgress',
      'collected.transcript',
      'collectedTitle',
      'description',
      'disclosure',
      'label',
      'osNote',
      'upgradeNotice.body',
      'upgradeNotice.cta',
      'upgradeNotice.dismiss',
      'upgradeNotice.title',
    ])
  })

  for (const [name, locale] of Object.entries(LOCALES)) {
    it(`${name} has an identical microphone key-set to en`, () => {
      expect(flatKeys(micBlock(locale as JsonRecord))).toEqual(reference)
    })

    it(`${name} microphone values are all non-empty strings`, () => {
      const block = micBlock(locale as JsonRecord)
      for (const path of reference) {
        const value = path.split('.').reduce<unknown>((acc, seg) => (acc as JsonRecord)[seg], block)
        expect(typeof value, `${name}.${path}`).toBe('string')
        expect((value as string).trim().length, `${name}.${path}`).toBeGreaterThan(0)
      }
    })
  }
})
