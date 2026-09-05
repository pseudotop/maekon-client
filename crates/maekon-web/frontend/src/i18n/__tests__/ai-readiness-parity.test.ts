import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

describe('AI readiness action localization (#11735)', () => {
  it('keeps every capability, reason, and action key available in all supported locales', () => {
    const referenceGroups = {
      capability: Object.keys(en.aiReadiness.capability).sort(),
      reason: Object.keys(en.aiReadiness.reason).sort(),
      action: Object.keys(en.aiReadiness.action).sort(),
    }

    for (const locale of [ko, ja, es, zhCN]) {
      expect(locale.aiReadiness.readyTitle.trim()).not.toBe('')
      expect(locale.aiReadiness.blockedTitle.trim()).not.toBe('')
      for (const group of Object.keys(referenceGroups) as Array<keyof typeof referenceGroups>) {
        expect(Object.keys(locale.aiReadiness[group]).sort()).toEqual(referenceGroups[group])
        for (const key of referenceGroups[group]) {
          const values = locale.aiReadiness[group] as Record<string, string>
          expect(values[key].trim()).not.toBe('')
        }
      }
    }
  })
})
