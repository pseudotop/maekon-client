export function formatLocalCalendarDate(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

export function parseLocalCalendarDate(value: string): Date | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value)
  if (!match) return undefined

  const [, year, month, day] = match
  const parsed = new Date(Number(year), Number(month) - 1, Number(day))
  if (Number.isNaN(parsed.getTime()) || formatLocalCalendarDate(parsed) !== value) return undefined
  return parsed
}

export function localCalendarDateFromValue(value?: string): string {
  if (!value) return ''
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) return parseLocalCalendarDate(value) ? value : ''

  const parsed = new Date(value)
  return Number.isNaN(parsed.getTime()) ? '' : formatLocalCalendarDate(parsed)
}

export function shiftLocalCalendarDate(value: string, days: number): string {
  const parsed = parseLocalCalendarDate(value)
  if (!parsed) return value

  parsed.setDate(parsed.getDate() + days)
  return formatLocalCalendarDate(parsed)
}

export function localDayBoundaryIso(calendarDate: string, boundary: 'start' | 'end'): string | undefined {
  const parsed = parseLocalCalendarDate(calendarDate)
  if (!parsed) return undefined

  if (boundary === 'end') {
    parsed.setDate(parsed.getDate() + 1)
    parsed.setMilliseconds(-1)
  }

  return parsed.toISOString()
}
