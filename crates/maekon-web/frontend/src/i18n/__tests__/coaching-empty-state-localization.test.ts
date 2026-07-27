import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const localizedCoaching = { es, ja, ko, 'zh-CN': zhCN }
const emptyStateKeys = ['noEventsHint', 'noGoalsHint'] as const

describe('coaching empty-state localization', () => {
  for (const [locale, resources] of Object.entries(localizedCoaching)) {
    it(`${locale} does not reuse English empty-state hints`, () => {
      for (const key of emptyStateKeys) {
        expect(resources.coaching[key]).not.toBe(en.coaching[key])
        expect(resources.coaching[key].trim()).not.toBe('')
      }
    })
  }
})
