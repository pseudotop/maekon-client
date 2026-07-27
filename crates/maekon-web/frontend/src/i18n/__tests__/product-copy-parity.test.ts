import { describe, expect, it } from 'vitest'

import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, es, ja, ko, 'zh-CN': zhCN } as const

const DELETE_TERMS = {
  en: /delete/i,
  es: /eliminar/i,
  ja: /削除/,
  ko: /삭제/,
  'zh-CN': /删除/,
} as const

const EXTERNAL_FILE_EXCEPTIONS = {
  en: /not removed/i,
  es: /no se eliminan/i,
  ja: /削除されません/,
  ko: /삭제되지 않습니다/,
  'zh-CN': /不会将其删除/,
} as const

const HISTORY_TERMS = {
  en: /history/i,
  es: /historial/i,
  ja: /履歴/,
  ko: /기록/,
  'zh-CN': /历史/,
} as const

describe('high-risk product copy parity', () => {
  it('keeps the consent-withdrawal contract structurally identical in all five locales', () => {
    const reference = Object.keys(en.privacy.consent.withdraw).sort()

    for (const [locale, resource] of Object.entries(LOCALES)) {
      expect(Object.keys(resource.privacy.consent.withdraw).sort(), locale).toEqual(reference)
    }
  })

  it('states deletion and the external-file exception in every consent-withdrawal locale', () => {
    for (const [locale, resource] of Object.entries(LOCALES)) {
      const localeName = locale as keyof typeof LOCALES
      const withdrawal = resource.privacy.consent.withdraw
      expect(`${withdrawal.label} ${withdrawal.confirmMessage} ${withdrawal.confirmButton}`, locale).toMatch(
        DELETE_TERMS[localeName],
      )
      expect(withdrawal.confirmNote, locale).toMatch(EXTERNAL_FILE_EXCEPTIONS[localeName])
    }
  })

  it('names the actual update-channel route instead of promising update history', () => {
    for (const [locale, resource] of Object.entries(LOCALES)) {
      const localeName = locale as keyof typeof LOCALES
      expect(resource.sidebar, locale).not.toHaveProperty('updateHistory')
      expect(resource.sidebar.updateChannel, locale).not.toMatch(HISTORY_TERMS[localeName])
    }
  })

  it('localizes recovery copy and install actions instead of leaking English fallbacks', () => {
    for (const [locale, resource] of Object.entries(LOCALES)) {
      expect(resource.updates.statusCheckFailed.trim(), locale).not.toBe('')
      expect(resource.updates.actionFailed.trim(), locale).not.toBe('')
      expect(resource.updates.technicalDetails.trim(), locale).not.toBe('')
      if (locale !== 'en') {
        expect(resource.updates.readyToInstallMsg, locale).not.toBe(en.updates.readyToInstallMsg)
        expect(resource.updates.installNow, locale).not.toBe(en.updates.installNow)
      }
    }
  })
})
