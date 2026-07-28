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

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources,
    fallbackLng: 'en',
    supportedLngs: ['ko', 'en', 'ja', 'zh-CN', 'es'],
    // #8058 P2-5: map a region-suffixed OS language to its supported base
    // (e.g. 'ko-KR' -> 'ko', 'ja-JP' -> 'ja', 'en-US' -> 'en') while keeping the
    // explicit region code 'zh-CN' an exact match. Without this, navigator values
    // like 'ko-KR' miss `supportedLngs` and fall through to the English fallback,
    // defeating OS-language detection.
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

// Get current language
export const getCurrentLanguage = (): SupportedLanguageCode => {
  const lng = i18n.language
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
