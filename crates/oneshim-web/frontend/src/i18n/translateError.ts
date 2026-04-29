/**
 * Wire-code → localized message translator for ADR-019 IpcError envelopes.
 *
 * Keys follow the 41 wire codes from
 * `crates/oneshim-core/tests/wire_contract_snapshot.expected.txt`.
 * Coverage is enforced at build time via
 * `scripts/check-error-i18n-coverage.ts`.
 *
 * This translator is deliberately NOT wired into react-i18next's resources.
 * Wire codes contain dots (`config.invalid`, `provider.bedrock.unsupported`)
 * which i18next treats as path separators; using a flat JSON lookup keeps
 * the keys literal and avoids accidental collisions with UI-namespaced
 * translation keys (`errors.boundaryTitle`, etc.).
 *
 * See:
 * - [ADR-019 Follow-up #3 design](../../../../docs/reviews/2026-04-20-adr019-followup-frontend-i18n-wiring-design.md)
 * - `src/api/desktop.ts` for the `IpcError` type + `isIpcError` guard.
 */

import { isIpcError } from '../api/desktop'

import enMessages from './wire-errors.en.json'
import koMessages from './wire-errors.ko.json'

type LocaleMap = Record<string, string>

/** Supported locales for wire-error translation. */
export type WireErrorLocale = 'en' | 'ko'

const resources: Record<WireErrorLocale, LocaleMap> = {
  en: enMessages as LocaleMap,
  ko: koMessages as LocaleMap,
}

/**
 * Translate an `IpcError` (or any unknown Tauri invoke rejection) into a
 * localized user-facing message.
 *
 * Resolution order:
 * 1. `IpcError` with a known wire code in the target locale → formatted template
 * 2. `IpcError` with a known wire code, but translation missing in target locale
 *    → fall back to English translation
 * 3. `IpcError` with an unknown wire code → fall back to raw `message`
 * 4. Plain string (pre-migration Tauri command) → return as-is
 * 5. Error instance → `error.message`
 * 6. Anything else → `String(err)`
 *
 * Template placeholders: `{message}` is the only substitution supported.
 * Used for per-call detail (e.g., which field failed validation).
 */
export function translateError(err: unknown, locale: WireErrorLocale = 'en'): string {
  if (isIpcError(err)) {
    const map = resources[locale] ?? resources.en
    const template = map[err.code] ?? resources.en[err.code] ?? err.message
    return template.replace('{message}', err.message)
  }
  if (typeof err === 'string') {
    return err
  }
  if (err instanceof Error) {
    return err.message
  }
  return String(err)
}

/**
 * Returns true when the wire code has a translation in the specified locale.
 * Useful for conditional UI (e.g., show a generic "Learn more" link only
 * for codes with translation coverage).
 */
export function hasTranslation(code: string, locale: WireErrorLocale = 'en'): boolean {
  return code in (resources[locale] ?? {})
}

/**
 * Returns the set of wire codes that have translations in the specified locale.
 * Exposed primarily for test + coverage-check tooling.
 */
export function translatedCodes(locale: WireErrorLocale = 'en'): readonly string[] {
  return Object.freeze(Object.keys(resources[locale] ?? {}))
}
