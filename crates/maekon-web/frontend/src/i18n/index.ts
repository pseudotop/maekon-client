// i18n setup
import i18n from 'i18next'
import LanguageDetector from 'i18next-browser-languagedetector'
import { initReactI18next } from 'react-i18next'
import en from './locales/en.json'
import es from './locales/es.json'
import ja from './locales/ja.json'
import ko from './locales/ko.json'
import zhCN from './locales/zh-CN.json'

const resources = {
  ko: { translation: ko },
  en: { translation: en },
  ja: { translation: ja },
  'zh-CN': { translation: zhCN },
  es: { translation: es },
}

// #13749: a Chinese tag other than 'zh-CN' ('zh-TW', 'zh-Hant-TW', bare 'zh').
// None of them has a bundle.
const isChineseWithoutBundle = (code: string) => /^zh(-|$)/i.test(code) && code.toLowerCase() !== 'zh-cn'

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources,
    fallbackLng: 'en',
    supportedLngs: ['ko', 'en', 'ja', 'zh-CN', 'zh', 'es'],
    // #8058 P2-5: map a region-suffixed OS language to its supported base
    // (e.g. 'ko-KR' -> 'ko', 'ja-JP' -> 'ja', 'en-US' -> 'en'). Without this,
    // navigator values like 'ko-KR' miss `supportedLngs` and fall through to the
    // English fallback, defeating OS-language detection.
    // #13749: with this option i18next strips the region from every code before
    // it looks the code up in `supportedLngs`, 'zh-CN' included: it looks for
    // 'zh'. The 'zh' entry above is what admits 'zh-CN'; it is not a sixth
    // locale (resources stay under 'zh-CN'). Without it i18next drops 'zh-CN'
    // from the lookup chain and the whole app renders the English fallback.
    // 'zh-CN' stays listed so that it still resolves if this option is turned off.
    nonExplicitSupportedLngs: true,

    detection: {
      // Explicit user choice (localStorage) always wins; on first run — before any
      // choice is stored — fall back to the OS/browser language via 'navigator'
      // (#8058 P2-5). Previously only 'localStorage' was consulted, so a fresh
      // install was pinned to English regardless of OS locale, inconsistent with
      // the theme surface which already honors OS `prefers-color-scheme`.
      order: ['localStorage', 'navigator'],
      lookupLocalStorage: 'maekon-language',
      caches: ['localStorage'],
      // #13749: the 'zh' entry in `supportedLngs` admits every Chinese tag, not
      // only 'zh-CN'. Detection would stop at 'zh-TW' and render the English
      // fallback instead of going on to the user's next language
      // (['zh-TW', 'ko-KR'] is Korean). An empty code matches nothing, so
      // i18next moves on.
      convertDetectedLanguage: (code: string) => (isChineseWithoutBundle(code) ? '' : code),
    },

    interpolation: {
      escapeValue: false, // React handles escaping
    },

    react: {
      useSuspense: false, // Used without SSR
    },
  })

export default i18n

export type SupportedLanguageCode = 'ko' | 'en' | 'ja' | 'zh-CN' | 'es'

// Language change helper
export const changeLanguage = (lng: SupportedLanguageCode) => {
  i18n.changeLanguage(lng)
  localStorage.setItem('maekon-language', lng)
}

// Get current language: the locale whose resources are on screen.
// #13749: `i18n.language` keeps the detected tag ('ko-KR'), which is not one of
// the selectable codes, so the language selector showed English for it.
export const getCurrentLanguage = (): SupportedLanguageCode => {
  const lng = i18n.resolvedLanguage
  return (['ko', 'en', 'ja', 'zh-CN', 'es'] as const).includes(lng as SupportedLanguageCode)
    ? (lng as SupportedLanguageCode)
    : 'en'
}

// Supported language list
export const supportedLanguages = [
  { code: 'en', name: 'English' },
  { code: 'ko', name: '한국어' },
  { code: 'ja', name: '日本語' },
  { code: 'zh-CN', name: '简体中文' },
  { code: 'es', name: 'Español' },
] as const
