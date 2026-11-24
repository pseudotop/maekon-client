/**
 * Shared display formatters.
 *
 * #13740: this module is the only place that turns seconds into minutes and
 * hours for display. Pages and components call these helpers instead of
 * dividing by 60 or 3600 themselves, and `no-direct-time-math.test.ts` keeps it
 * that way.
 */

import i18next from 'i18next'

const SECONDS_PER_MINUTE = 60
const MINUTES_PER_HOUR = 60
const SECONDS_PER_HOUR = SECONDS_PER_MINUTE * MINUTES_PER_HOUR
const HOURS_PER_DAY = 24
const MS_PER_MINUTE = SECONDS_PER_MINUTE * 1000

/** Whole seconds; negative and non-finite input count as zero. */
function wholeSeconds(secs: number): number {
  return Number.isFinite(secs) && secs > 0 ? Math.floor(secs) : 0
}

/**
 * Reads a `common.duration.*` pattern from the shared i18next instance, the
 * same one `useTranslation()` uses. Falls back to the English pattern before
 * i18next is ready so a caller never gets `undefined` or a raw key.
 */
function durationText(key: string, values: Record<string, number>, fallback: string): string {
  const text = i18next.isInitialized ? i18next.t(`common.duration.${key}`, values) : undefined
  return typeof text === 'string' && text !== `common.duration.${key}` ? text : fallback
}

/**
 * The one duration format for every screen (#13740).
 *
 * - under a minute: seconds ("45s")
 * - under an hour: whole minutes, seconds dropped ("7m")
 * - an hour or more: hours and minutes ("6h 7m"), hours alone on the hour ("6h")
 *
 * Units follow the active language, so a Korean screen shows the same value
 * with the Korean units from `common.duration.*`.
 */
export function formatDuration(secs: number): string
/** @deprecated There is a single duration format now; the second argument is ignored (#13740). */
export function formatDuration(secs: number, short: boolean): string
export function formatDuration(secs: number): string {
  const total = wholeSeconds(secs)
  if (total < SECONDS_PER_MINUTE) {
    return durationText('seconds', { count: total }, `${total}s`)
  }
  if (total < SECONDS_PER_HOUR) {
    const minutes = Math.floor(total / SECONDS_PER_MINUTE)
    return durationText('minutes', { count: minutes }, `${minutes}m`)
  }
  const hours = Math.floor(total / SECONDS_PER_HOUR)
  const minutes = Math.floor((total % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE)
  if (minutes === 0) {
    return durationText('hours', { count: hours }, `${hours}h`)
  }
  return durationText('hoursMinutes', { hours, minutes }, `${hours}h ${minutes}m`)
}

/** `formatDuration` for a value the API counts in minutes, such as a segment's `duration_mins`. */
export function formatMinutes(mins: number): string {
  return formatDuration(Math.round(mins * SECONDS_PER_MINUTE))
}

/**
 * `formatDuration` for a value the API counts in fractional hours, such as
 * `deep_work_hours`. Rounds to the nearest second first so 2.3333… hours reads
 * "2h 20m", not "2h 19m".
 */
export function formatHours(hours: number): string {
  return formatDuration(Math.round(hours * SECONDS_PER_HOUR))
}

/** Countdown clock "MM:SS". Minutes keep counting past 59 ("125:00"). */
export function formatClock(secs: number): string {
  const total = wholeSeconds(secs)
  const minutes = Math.floor(total / SECONDS_PER_MINUTE)
  const seconds = total % SECONDS_PER_MINUTE
  return `${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`
}

/** Nearest whole minute, for controls and copy whose unit is minutes (inputs, interval presets). */
export function minutesFromSeconds(secs: number): number {
  return Math.round(wholeSeconds(secs) / SECONDS_PER_MINUTE)
}

/** Fractional hours in `secs`, for comparing a seconds total with a value the API counts in hours (#13738). */
export function hoursFromSeconds(secs: number): number {
  return wholeSeconds(secs) / SECONDS_PER_HOUR
}

export type ElapsedUnit = 'minutes' | 'hours' | 'days'

/**
 * The largest whole unit of a past interval, for "N minutes/hours/days ago"
 * copy. Anything under a minute counts as one minute so the copy never reads
 * "0 minutes ago".
 */
export function largestElapsedUnit(elapsedMs: number): { unit: ElapsedUnit; count: number } {
  const elapsed = Number.isFinite(elapsedMs) && elapsedMs > 0 ? elapsedMs : 0
  const minutes = Math.max(1, Math.floor(elapsed / MS_PER_MINUTE))
  if (minutes < MINUTES_PER_HOUR) return { unit: 'minutes', count: minutes }
  const hours = Math.floor(minutes / MINUTES_PER_HOUR)
  if (hours < HOURS_PER_DAY) return { unit: 'hours', count: hours }
  return { unit: 'days', count: Math.floor(hours / HOURS_PER_DAY) }
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)}MB`
  return formatGigabytes(bytes / (1024 * 1024 * 1024))
}

export function formatGigabytes(gigabytes: number): string {
  return `${gigabytes.toFixed(1)}GB`
}

const DEFAULT_LOCALE = 'en-US'

function resolveLocale(locale?: string): string {
  return locale || DEFAULT_LOCALE
}

export function formatTime(timestamp: string, locale?: string): string {
  const date = new Date(timestamp)
  return date.toLocaleTimeString(resolveLocale(locale), {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  })
}

export function formatDateTime(timestamp: string, locale?: string): string {
  const date = new Date(timestamp)
  return date.toLocaleString(resolveLocale(locale), {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

export function formatDate(timestamp: string | undefined, locale?: string): string {
  if (!timestamp) return new Date().toISOString().split('T')[0]
  const date = new Date(timestamp)
  return date.toLocaleDateString(resolveLocale(locale), { month: 'short', day: 'numeric' })
}

export function formatNumber(num: number, locale?: string): string {
  return num.toLocaleString(resolveLocale(locale))
}

export function formatHour(hourStr: string): string {
  const hour = parseInt(hourStr, 10)
  if (Number.isNaN(hour)) return hourStr
  return `${hour}시`
}

export function formatPercent(value: number, decimals = 1): string {
  return `${value.toFixed(decimals)}%`
}

export function escapeRegex(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}
