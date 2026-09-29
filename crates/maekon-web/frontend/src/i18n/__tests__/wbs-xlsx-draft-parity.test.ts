import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const
type JsonRecord = Record<string, unknown>

function flatKeys(obj: JsonRecord, prefix = ''): string[] {
  return Object.entries(obj)
    .flatMap(([key, value]) => {
      const path = prefix ? `${prefix}.${key}` : key
      return value && typeof value === 'object' && !Array.isArray(value) ? flatKeys(value as JsonRecord, path) : [path]
    })
    .sort()
}

describe('WBS XLSX draft i18n parity (5 locales)', () => {
  const reference = flatKeys(en.wbsXlsxDraft as JsonRecord)

  for (const [name, locale] of Object.entries(LOCALES)) {
    it(`${name} has the complete route and surface key set`, () => {
      expect(flatKeys(locale.wbsXlsxDraft as JsonRecord)).toEqual(reference)
      expect(locale.nav.wbsXlsxDraft.trim()).not.toBe('')
      expect(locale.wbsXlsxDraft.synthetic.trim()).not.toBe('')
      expect(locale.wbsXlsxDraft.syntheticAria.trim()).not.toBe('')
    })
  }
})
