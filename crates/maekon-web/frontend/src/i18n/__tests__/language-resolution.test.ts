/**
 * Language resolution of the real i18n config (#13749).
 *
 * Every other locale test builds its own i18next instance from the JSON files,
 * so none of them notices when the shipped `init` options stop a locale from
 * resolving. These cases import `../index` itself, once per detection input.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const
type Code = keyof typeof LOCALES
const CODES = Object.keys(LOCALES) as Code[]
const STORAGE_KEY = 'maekon-language'

// The probe key must read differently in every locale: a locale that renders
// another locale's bundle (the English fallback included) then cannot pass.
const PROBE_KEY = 'settings.language'
const probeText = (code: Code) => LOCALES[code].settings.language

async function loadI18n(input: { stored?: string; osLanguages: string[] }) {
  localStorage.clear()
  if (input.stored !== undefined) localStorage.setItem(STORAGE_KEY, input.stored)
  vi.spyOn(window.navigator, 'languages', 'get').mockReturnValue(input.osLanguages)
  vi.spyOn(window.navigator, 'language', 'get').mockReturnValue(input.osLanguages[0] ?? '')
  vi.resetModules()
  return import('../index')
}

afterEach(() => {
  vi.restoreAllMocks()
  localStorage.clear()
})

describe('i18n language resolution', () => {
  it('uses a probe key that differs in every shipped locale', () => {
    expect(new Set(CODES.map(probeText)).size).toBe(CODES.length)
  })

  it('offers exactly the locales that have a resource bundle', async () => {
    const { supportedLanguages } = await loadI18n({ osLanguages: ['en-US', 'en'] })
    expect(supportedLanguages.map((language) => language.code).sort()).toEqual([...CODES].sort())
  })

  describe('explicit choice', () => {
    it.each(CODES)('renders %s after it is selected', async (code) => {
      // Start from a different language so the selection has to change it.
      const start = code === 'en' ? 'ko-KR' : 'en-US'
      const { default: i18n, changeLanguage, getCurrentLanguage } = await loadI18n({ osLanguages: [start] })

      changeLanguage(code)

      expect(i18n.t(PROBE_KEY)).toBe(probeText(code))
      expect(getCurrentLanguage()).toBe(code)
      expect(localStorage.getItem(STORAGE_KEY)).toBe(code)
    })

    it.each(CODES)('renders stored %s on the next launch, over the OS language', async (code) => {
      const os = code === 'en' ? 'ko-KR' : 'en-US'
      const { default: i18n, getCurrentLanguage } = await loadI18n({ stored: code, osLanguages: [os] })

      expect(i18n.t(PROBE_KEY)).toBe(probeText(code))
      expect(getCurrentLanguage()).toBe(code)
    })
  })

  describe('OS language on first run', () => {
    it.each<[string[], Code]>([
      [['ko-KR'], 'ko'],
      [['ko-KR', 'ko', 'en-US', 'en'], 'ko'],
      // A later exact match must not outrank an earlier region-suffixed one.
      [['ko-KR', 'en-US', 'en'], 'ko'],
      [['en-US', 'en'], 'en'],
      [['en-GB'], 'en'],
      [['ja-JP'], 'ja'],
      [['ja', 'en-US', 'en'], 'ja'],
      [['es-MX'], 'es'],
      [['es-419', 'es'], 'es'],
      [['zh-CN'], 'zh-CN'],
      [['zh-CN', 'zh'], 'zh-CN'],
      [['zh-CN', 'zh', 'en-US', 'en'], 'zh-CN'],
      // Language tags are case-insensitive (BCP 47).
      [['zh-cn'], 'zh-CN'],
      // Between two supported languages the order of the list decides.
      [['zh-CN', 'ko-KR'], 'zh-CN'],
      [['ko-KR', 'zh-CN'], 'ko'],
      // Unsupported languages are skipped, not treated as English.
      [['fr-FR', 'ko-KR'], 'ko'],
      [['fr-FR', 'fr'], 'en'],
      [[], 'en'],
      // Chinese tags other than zh-CN have no bundle: they are skipped like any
      // unsupported language, so the next language in the list wins.
      [['zh-TW', 'ko-KR', 'ko'], 'ko'],
      [['zh-TW', 'zh', 'ja', 'en-US', 'en'], 'ja'],
      [['zh-HK', 'es-ES'], 'es'],
      [['zh-TW', 'zh-CN'], 'zh-CN'],
      [['zh-TW'], 'en'],
      [['zh'], 'en'],
    ])('%j renders %s', async (osLanguages, expected) => {
      const { default: i18n, getCurrentLanguage } = await loadI18n({ osLanguages })

      expect(i18n.t(PROBE_KEY)).toBe(probeText(expected))
      // The language selector reads this value: it has to name the language on screen.
      expect(getCurrentLanguage()).toBe(expected)
    })
  })
})
