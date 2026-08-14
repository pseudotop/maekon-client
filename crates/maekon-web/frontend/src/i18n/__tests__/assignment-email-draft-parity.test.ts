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

describe('assignment email draft i18n parity (5 locales)', () => {
  const reference = flatKeys(en.assignmentEmailDraft as JsonRecord)

  for (const [name, locale] of Object.entries(LOCALES)) {
    it(`${name} has the complete non-empty key set and route label`, () => {
      const block = locale.assignmentEmailDraft as JsonRecord
      expect(flatKeys(block)).toEqual(reference)
      for (const value of Object.values(block)) {
        expect(value).toBeDefined()
      }
      expect(locale.nav.assignmentEmailDraft.trim()).not.toBe('')
    })
  }
})
