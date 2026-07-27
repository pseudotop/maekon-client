import { describe, expect, it } from 'vitest'
import { clampCpuPercent, floorNonNegative } from '../MetricsChart'

// #8082 CJ-05-11: implausible CPU/memory samples must be bounded before they
// reach the chart, not rendered as valid readings.
describe('MetricsChart value bounds (#8082)', () => {
  it('clamps CPU above 100% down to the 100% ceiling', () => {
    // Multi-core aggregate / corrupt sample that summed per-core busy time.
    expect(clampCpuPercent(340)).toBe(100)
    expect(clampCpuPercent(100.0001)).toBe(100)
  })

  it('clamps negative CPU up to 0', () => {
    expect(clampCpuPercent(-5)).toBe(0)
  })

  it('passes plausible CPU values through unchanged', () => {
    expect(clampCpuPercent(0)).toBe(0)
    expect(clampCpuPercent(42.5)).toBe(42.5)
    expect(clampCpuPercent(100)).toBe(100)
  })

  it('coerces non-finite CPU samples to 0 rather than NaN', () => {
    expect(clampCpuPercent(Number.NaN)).toBe(0)
    expect(clampCpuPercent(undefined)).toBe(0)
    expect(clampCpuPercent('not-a-number')).toBe(0)
  })

  it('floors memory at 0 but keeps its real (unbounded-above) magnitude', () => {
    expect(floorNonNegative(-1024)).toBe(0)
    expect(floorNonNegative(8 * 1024 * 1024 * 1024)).toBe(8 * 1024 * 1024 * 1024)
    expect(floorNonNegative(Number.NaN)).toBe(0)
  })
})
