import type { DeleteRangeRequest } from '../../api/client'

const DATE_INPUT_PATTERN = /^\d{4}-\d{2}-\d{2}$/

function localMidnight(date: string): Date {
  if (!DATE_INPUT_PATTERN.test(date)) {
    throw new Error(`Invalid date input: ${date}`)
  }

  const midnight = new Date(`${date}T00:00:00.000`)
  if (Number.isNaN(midnight.getTime())) {
    throw new Error(`Invalid date input: ${date}`)
  }

  return midnight
}

/** Convert local calendar-day inputs into the API's closed RFC3339 interval. */
export function buildDeleteRangeRequest(
  fromDate: string,
  toDate: string,
  selectedDataTypes: readonly string[],
): DeleteRangeRequest {
  const from = localMidnight(fromDate)
  const nextLocalMidnight = localMidnight(toDate)
  nextLocalMidnight.setDate(nextLocalMidnight.getDate() + 1)

  const finalMillisecond = new Date(nextLocalMidnight.getTime() - 1).toISOString()
  const inclusiveEnd = finalMillisecond.replace(/\.999Z$/, '.999999999Z')

  return {
    from: from.toISOString(),
    to: inclusiveEnd,
    data_types: selectedDataTypes.length > 0 ? [...selectedDataTypes] : undefined,
  }
}
