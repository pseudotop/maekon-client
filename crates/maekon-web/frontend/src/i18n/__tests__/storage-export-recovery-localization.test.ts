import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const locales = { en, es, ja, ko, 'zh-CN': zhCN }
const recoveryKeys = ['exportFailed', 'exportStorageRecovery', 'exportGenericRecovery', 'exportRetry'] as const

describe('storage export recovery localization', () => {
  it.each(Object.entries(locales))('%s includes every recovery key', (_locale, resource) => {
    for (const key of recoveryKeys) {
      expect(resource.settings[key].trim(), `missing settings.${key}`).not.toBe('')
    }
  })

  it.each(
    Object.entries(locales).filter(([locale]) => locale !== 'en'),
  )('%s does not reuse the English recovery guidance', (_locale, resource) => {
    expect(resource.settings.exportStorageRecovery).not.toBe(en.settings.exportStorageRecovery)
    expect(resource.settings.exportGenericRecovery).not.toBe(en.settings.exportGenericRecovery)
  })
})
