/**
 * #13738 — one Korean word per concept.
 *
 * Korean screens used to call the same thing by several names: provider had
 * four spellings (the English word, two transliterations and a native word),
 * regime three, and segment two, so one feature read differently on two
 * screens. The provider connection surface sat in the same sentences as an
 * untranslated "surface". `TERMS` below holds the one chosen word for each
 * concept and the spellings it replaced.
 *
 * Two checks keep it that way. No ko value may use a replaced spelling, and a
 * ko value whose English text uses the concept must use the chosen word.
 * Interpolation placeholders are ignored: `{{provider}}` is a variable name,
 * not copy.
 */

import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import ko from '../locales/ko.json'
import wireErrorsKo from '../wire-errors.ko.json'

type JsonRecord = Record<string, unknown>

function flatten(obj: JsonRecord, prefix = '', out: Record<string, string> = {}): Record<string, string> {
  for (const [k, v] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${k}` : k
    if (v !== null && typeof v === 'object' && !Array.isArray(v)) {
      flatten(v as JsonRecord, path, out)
    } else if (typeof v === 'string') {
      out[path] = v
    }
  }
  return out
}

/** Copy without `{{name}}` (i18next) and `{name}` (wire error) placeholders. */
function copyText(value: string): string {
  return value.replace(/\{\{[^}]*\}\}/g, '').replace(/\{[^}]*\}/g, '')
}

interface Term {
  readonly concept: string
  readonly chosen: string
  /** Other spellings that must not appear in Korean copy. */
  readonly forbidden: readonly RegExp[]
  /** English wording that means this concept. */
  readonly english: RegExp
}

const TERMS: readonly Term[] = [
  {
    concept: 'provider',
    chosen: '제공자',
    forbidden: [/provider/i, /프로바이더/, /공급자/],
    english: /\bproviders?\b/i,
  },
  // The lookbehind keeps the Korean word for "operating system", which ends in the same two syllables.
  { concept: 'regime', chosen: '활동 유형', forbidden: [/regime/i, /레짐/, /(?<!운영)체제/], english: /\bregimes?\b/i },
  { concept: 'segment', chosen: '구간', forbidden: [/segment/i, /세그먼트/], english: /\bsegments?\b/i },
  { concept: 'surface', chosen: '연결 방식', forbidden: [/surface/i], english: /\bsurfaces?\b/i },
]

/**
 * English copy that uses "surface" for a screen area rather than a provider
 * connection, so the Korean text rightly does not use the connection word.
 */
const OTHER_SENSE: Readonly<Record<string, string>> = {
  'settings.guidance.audio.description': 'storage surface: how much is captured and stored',
  'privacy.capturePauseDesc': 'privacy surface: the privacy screen',
  'trayDashboard.redaction.blocked': 'sensitive surface: a sensitive window on screen',
}

const flatEn = flatten(en as JsonRecord)
const koCopy: Record<string, string> = {
  ...flatten(ko as JsonRecord),
  ...Object.fromEntries(
    Object.entries(flatten(wireErrorsKo as JsonRecord)).map(([key, value]) => [`wire-errors:${key}`, value]),
  ),
}

describe('Korean terminology (#13738)', () => {
  it.each(TERMS)('uses only $chosen for $concept', ({ forbidden }) => {
    const offenders = Object.entries(koCopy)
      .filter(([, value]) => forbidden.some((pattern) => pattern.test(copyText(value))))
      .map(([key, value]) => `${key}: ${value}`)
    expect(offenders).toEqual([])
  })

  it.each(TERMS)('says $chosen wherever the English copy names $concept', ({ concept, chosen, english }) => {
    const anchored = Object.entries(flatEn).filter(
      ([key, value]) => english.test(copyText(value)) && !(concept === 'surface' && key in OTHER_SENSE),
    )
    // Positive control: the English side still uses the concept, so a pass is not an empty scan.
    expect(anchored.length).toBeGreaterThan(0)
    const missing = anchored
      .filter(([key]) => !(koCopy[key] ?? '').includes(chosen))
      .map(([key]) => `${key}: ${koCopy[key]}`)
    expect(missing).toEqual([])
  })

  it('keeps the other-sense exceptions honest', () => {
    for (const key of Object.keys(OTHER_SENSE)) {
      expect(flatEn[key], key).toMatch(/\bsurfaces?\b/i)
      expect(koCopy[key], key).not.toContain('연결 방식')
    }
  })
})
