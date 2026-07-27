import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const locales = { en, es, ja, ko, 'zh-CN': zhCN }

describe('update stream recovery localization', () => {
  it.each(Object.entries(locales))('%s includes recovery status copy', (_locale, resource) => {
    expect(resource.updates.streamRecovered.trim()).not.toBe('')
  })

  it.each(
    Object.entries(locales).filter(([locale]) => locale !== 'en'),
  )('%s does not reuse the English recovery status', (_locale, resource) => {
    expect(resource.updates.streamRecovered).not.toBe(en.updates.streamRecovered)
  })
})
