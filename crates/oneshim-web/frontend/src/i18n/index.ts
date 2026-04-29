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

    detection: {
      order: ['localStorage'],
      lookupLocalStorage: 'oneshim-language',
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
  localStorage.setItem('oneshim-language', lng)
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
