import { describe, expect, it } from 'vitest'
import { buildDeleteRangeRequest } from './deleteRangeRequest'

describe('buildDeleteRangeRequest', () => {
  it('converts local calendar dates to an inclusive RFC3339 interval', () => {
    const request = buildDeleteRangeRequest('2026-07-13', '2026-07-13', ['events'])
    const expectedStart = new Date(2026, 6, 13, 0, 0, 0, 0)
    const nextLocalMidnight = new Date(2026, 6, 14, 0, 0, 0, 0)

    expect(request.from).toBe(expectedStart.toISOString())
    expect(request.to).toBe(new Date(nextLocalMidnight.getTime() - 1).toISOString().replace(/\.999Z$/, '.999999999Z'))
    expect(request.from).toMatch(/T\d{2}:\d{2}:\d{2}\.000Z$/)
    expect(request.to).toMatch(/T\d{2}:\d{2}:\d{2}\.999999999Z$/)
    expect(request.data_types).toEqual(['events'])
  })

  it('omits data_types when the UI selection means all types', () => {
    const request = buildDeleteRangeRequest('2026-07-12', '2026-07-13', [])

    expect(request.data_types).toBeUndefined()
  })
})
