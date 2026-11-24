/**
 * #13740: 5-locale parity for the keys the time-format change owns:
 * `common.*` (including the `common.duration.*` units), `updates.*` and
 * `statusBar.*`.
 *
 * Every locale must carry the same keys with the same `{{placeholders}}`, the
 * duration units must read naturally in each language, and no key may keep the
 * i18next v3 `_plural` suffix, which i18next 23 never reads (it uses `_one` /
 * `_other`), so English used to say "released 5 minute ago".
 */

import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

type JsonRecord = Record<string, unknown>

const LOCALES: Record<string, JsonRecord> = { en, ko, ja, 'zh-CN': zhCN, es }
const NAMESPACES = ['common', 'updates', 'statusBar'] as const

function flatten(obj: JsonRecord, prefix = '', out: Record<string, string> = {}): Record<string, string> {
  for (const [key, value] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${key}` : key
    if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
      flatten(value as JsonRecord, path, out)
    } else {
      out[path] = String(value)
    }
  }
  return out
}

function placeholders(text: string): string[] {
  return [...new Set([...text.matchAll(/\{\{\s*([^}\s]+)\s*\}\}/g)].map((match) => match[1]))].sort()
}

function fill(pattern: string, values: Record<string, number>): string {
  return pattern.replace(/\{\{\s*([^}\s]+)\s*\}\}/g, (_, name: string) => String(values[name]))
}

describe('time-format i18n parity (5 locales, #13740)', () => {
  for (const namespace of NAMESPACES) {
    const reference = flatten(en[namespace] as JsonRecord)

    for (const [locale, resource] of Object.entries(LOCALES)) {
      it(`${locale} ${namespace}.* has the same keys and placeholders as en`, () => {
        const actual = flatten(resource[namespace] as JsonRecord)
        expect(Object.keys(actual).sort()).toEqual(Object.keys(reference).sort())
        const mismatched = Object.keys(reference).filter(
          (key) => key in actual && placeholders(actual[key]).join() !== placeholders(reference[key]).join(),
        )
        expect(mismatched).toEqual([])
      })
    }
  }

  it('drops the i18next v3 _plural suffix that i18next 23 never reads', () => {
    for (const [locale, resource] of Object.entries(LOCALES)) {
      const legacy = NAMESPACES.flatMap((namespace) =>
        Object.keys(flatten(resource[namespace] as JsonRecord)).filter((key) => key.endsWith('_plural')),
      )
      expect(legacy, locale).toEqual([])
    }
  })

  it('spells 6h 7m, 45s and 7m in each language', () => {
    const expected: Record<string, [hoursMinutes: string, seconds: string, minutes: string, hours: string]> = {
      en: ['6h 7m', '45s', '7m', '6h'],
      ko: ['6시간 7분', '45초', '7분', '6시간'],
      ja: ['6時間7分', '45秒', '7分', '6時間'],
      'zh-CN': ['6小时7分钟', '45秒', '7分钟', '6小时'],
      es: ['6 h 7 min', '45 s', '7 min', '6 h'],
    }
    for (const [locale, resource] of Object.entries(LOCALES)) {
      const duration = flatten((resource.common as JsonRecord).duration as JsonRecord)
      expect(
        [
          fill(duration.hoursMinutes, { hours: 6, minutes: 7 }),
          fill(duration.seconds, { count: 45 }),
          fill(duration.minutes, { count: 7 }),
          fill(duration.hours, { count: 6 }),
        ],
        locale,
      ).toEqual(expected[locale])
    }
  })
})
