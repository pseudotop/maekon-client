/**
 * One display contract for every failure a screen shows (#13737).
 *
 * Backend diagnostics stay English by rule (ADR-019), and the frontend's own
 * `throw new Error('Tag query failed')` literals are English too, so a raw
 * `Error.message` or `IpcError.message` is a sentence for a log, not for a
 * Korean user. A screen renders `ErrorDisplay.text` and hands `detail` to
 * `SupportDetails`, which keeps the untranslated code and message one click
 * away for support without putting them in the user's way.
 *
 * Resolution order, mirroring `describeIpcError`:
 * 1. An out-of-registry Tauri code with a UI key (`TAURI_IPC_ERROR_KEYS`, or
 *    the caller's `lookup`) → that key's copy.
 * 2. An ADR-019 registry code with a catalog template → the template's
 *    localized sentence. Every catalog template ends in its `{message}` slot,
 *    and that slot carries the backend's English detail, so the sentence is
 *    shown without it and the detail moves to `detail`.
 * 3. A `LocalizedError` → its message, which the frontend already localized.
 * 4. Anything else — an unknown code, a plain `Error`, a legacy string — →
 *    the caller's fallback sentence, with the raw text kept as `detail`.
 *
 * `__tests__/raw-error-text-guard.test.ts` covers pages, components, overlay,
 * hooks and tracking-panel so indirect notice producers use this path too.
 */

import { useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import { type IpcError, isIpcError } from '../api/desktop'
import { TAURI_IPC_ERROR_KEYS, wireLocaleFor } from './tauriIpcErrors'
import { wireTemplate } from './translateError'

export interface ErrorDisplay {
  /** Localized sentence for the surface: a toast, an alert body, an inline line. */
  readonly text: string
  /** Untranslated code and message for support; null when `text` already names the cause. */
  readonly detail: string | null
}

/** Copy used when a caller has no more specific fallback sentence. */
export const GENERIC_ERROR_KEY = 'errors.generic'

/**
 * An error whose message the frontend already wrote in the active UI language,
 * e.g. `throw new LocalizedError(t('guiPicker.selectCandidate'))` inside a
 * mutation. The display path shows it as-is instead of the fallback.
 */
export class LocalizedError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'LocalizedError'
  }
}

const MESSAGE_SLOT = /\s*[:：]?\s*\{message\}\s*$/u

/**
 * The untranslated code and message of a failure: what `SupportDetails` shows,
 * and what a surface may pattern-match to pick a more specific sentence (a
 * port conflict, a missing credential). It is English diagnostic text, so it is
 * never the primary text of a surface.
 */
export function supportDetailOf(err: unknown): string | null {
  if (isIpcError(err)) {
    const message = err.message.trim()
    return message ? `${err.code}: ${message}` : err.code
  }
  if (typeof err === 'string') return err.trim() || null
  if (err instanceof Error) {
    const message = err.message.trim()
    if (!message) return err.name || null
    return err.name && err.name !== 'Error' ? `${err.name}: ${message}` : message
  }
  if (typeof err === 'object' && err !== null && 'message' in err) {
    const message = (err as { message: unknown }).message
    return typeof message === 'string' ? message.trim() || null : null
  }
  return null
}

/**
 * Localize an unknown failure for display.
 *
 * `fallback` is the already-translated sentence for "this action failed",
 * typically the surface's own key (`t('policies.createError')`). Without one,
 * the generic `errors.generic` copy is used.
 */
export function describeErrorForDisplay(
  err: unknown,
  translate: (key: string) => string,
  language: string | undefined,
  fallback?: string,
  lookup: (err: IpcError) => string | undefined = (value) => TAURI_IPC_ERROR_KEYS[value.code],
): ErrorDisplay {
  if (isIpcError(err)) {
    const key = lookup(err)
    if (key) return { text: translate(key), detail: null }
    const template = wireTemplate(err.code, wireLocaleFor(language))
    if (template !== undefined) {
      if (!template.includes('{message}')) return { text: template, detail: null }
      return { text: template.replace(MESSAGE_SLOT, ''), detail: supportDetailOf(err) }
    }
  }
  if (err instanceof LocalizedError && err.message.trim()) {
    return { text: err.message, detail: null }
  }
  return { text: fallback ?? translate(GENERIC_ERROR_KEY), detail: supportDetailOf(err) }
}

/**
 * Hook form of `describeErrorForDisplay` bound to the active UI language.
 *
 * ```ts
 * const describeError = useErrorDisplay()
 * onError: (error) => addErrorToast(describeError(error, t('policies.createError')))
 * ```
 */
export function useErrorDisplay(): (err: unknown, fallback?: string) => ErrorDisplay {
  const { t, i18n } = useTranslation()
  return useCallback(
    (err: unknown, fallback?: string) => describeErrorForDisplay(err, (key) => t(key), i18n?.language, fallback),
    [t, i18n?.language],
  )
}
