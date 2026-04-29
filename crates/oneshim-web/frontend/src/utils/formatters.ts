/**
 *
 */

export function formatDuration(secs: number, short = false): string {
  const hours = Math.floor(secs / 3600)
  const minutes = Math.floor((secs % 3600) / 60)
  const seconds = secs % 60

  if (short) {
    if (hours > 0) return `${hours}h ${minutes}m`
    return `${minutes}m`
  }

  if (hours > 0) return `${hours}h ${minutes}m ${seconds}s`
  if (minutes > 0) return `${minutes}m ${seconds}s`
  return `${seconds}s`
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
