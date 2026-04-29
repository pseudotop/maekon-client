import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

type LocaleTree = Record<string, unknown>

const currentDir = path.dirname(fileURLToPath(import.meta.url))
const localesDir = path.resolve(currentDir, '../../src/i18n/locales')

const koLocale = loadLocale('ko')
const enLocale = loadLocale('en')
// Only include locales with complete key coverage for shell/sidebar/shortcuts
const locales = [koLocale, enLocale]

function loadLocale(code: 'ko' | 'en'): LocaleTree {
  const localePath = path.join(localesDir, `${code}.json`)
  return JSON.parse(fs.readFileSync(localePath, 'utf-8')) as LocaleTree
}

function getByDottedPath(target: unknown, dottedPath: string): unknown {
  return dottedPath.split('.').reduce<unknown>((acc, segment) => {
    if (!acc || typeof acc !== 'object') {
      return undefined
    }
    return (acc as Record<string, unknown>)[segment]
  }, target)
}

function escapeRegex(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function unique(values: string[]): string[] {
  return [...new Set(values.map((value) => value.trim()).filter(Boolean))]
}

export function i18nTexts(...keys: string[]): string[] {
  const resolved = keys.flatMap((key) => locales.map((locale) => getByDottedPath(locale, key)))

  const texts = unique(resolved.filter((value): value is string => typeof value === 'string'))

  if (texts.length === 0) {
    throw new Error(`No i18n text found for keys: ${keys.join(', ')}`)
  }

  return texts
}

export function i18nRegex(keys: string | string[], extraLiterals: string[] = []): RegExp {
  const keyList = Array.isArray(keys) ? keys : [keys]
  const combined = unique([...i18nTexts(...keyList), ...extraLiterals])

  if (combined.length === 0) {
    throw new Error('No text available to build i18n regex')
  }

  return new RegExp(combined.map(escapeRegex).join('|'), 'i')
}
