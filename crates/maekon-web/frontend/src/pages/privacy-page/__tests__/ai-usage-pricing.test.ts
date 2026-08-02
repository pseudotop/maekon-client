/**
 * #9466: local price-table estimation unit tests — pricing must be a pure
 * local computation (no network), unknown models must yield null (token
 * counts still render), and formatting must keep sub-cent amounts legible.
 */

import { describe, expect, it } from 'vitest'
import { estimateSpendUsd, formatUsd, MODEL_PRICING } from '../ai-usage-pricing'

describe('estimateSpendUsd', () => {
  it('prices a known model per million tokens (input + output split)', () => {
    const est = estimateSpendUsd('claude-sonnet-5', 2_000_000, 1_000_000)
    expect(est).not.toBeNull()
    // 2M input × $3/M + 1M output × $15/M = $21
    expect(est?.usd).toBeCloseTo(21, 6)
  })

  it('matches case-insensitively and by substring (provider-prefixed ids)', () => {
    const est = estimateSpendUsd('us.anthropic.CLAUDE-HAIKU-4-5', 1_000_000, 0)
    expect(est?.usd).toBeCloseTo(0.8, 6)
  })

  it('returns null for an unknown model and for a missing model', () => {
    expect(estimateSpendUsd('mystery-llm-9000', 1_000_000, 1_000_000)).toBeNull()
    expect(estimateSpendUsd(null, 1_000_000, 1_000_000)).toBeNull()
  })

  it('prices local runtimes at zero', () => {
    const est = estimateSpendUsd('ollama/llama3', 5_000_000, 5_000_000)
    expect(est?.usd).toBe(0)
  })

  it('first (more specific) table entry wins over generic family entries', () => {
    // gpt-4o-mini must NOT be priced as gpt-4o.
    const mini = estimateSpendUsd('gpt-4o-mini', 1_000_000, 0)
    const full = estimateSpendUsd('gpt-4o', 1_000_000, 0)
    expect(mini?.usd).toBeCloseTo(0.15, 6)
    expect(full?.usd).toBeCloseTo(2.5, 6)
    const miniIndex = MODEL_PRICING.findIndex((p) => p.match === 'gpt-4o-mini')
    const fullIndex = MODEL_PRICING.findIndex((p) => p.match === 'gpt-4o')
    expect(miniIndex).toBeLessThan(fullIndex)
  })

  it('gpt-5 mini/nano variants are not priced at the flagship rate', () => {
    // Review finding on #9515: without their own rows ahead of the generic
    // 'gpt-5' entry, these substring-match to the flagship price — an
    // order-of-magnitude error on a cost-truth surface.
    expect(estimateSpendUsd('gpt-5-mini', 1_000_000, 1_000_000)?.usd).toBeCloseTo(2.25, 6)
    expect(estimateSpendUsd('gpt-5-nano', 1_000_000, 1_000_000)?.usd).toBeCloseTo(0.45, 6)
    expect(estimateSpendUsd('gpt-5', 1_000_000, 1_000_000)?.usd).toBeCloseTo(11.25, 6)
    const flagshipIndex = MODEL_PRICING.findIndex((p) => p.match === 'gpt-5')
    for (const variant of ['gpt-5-mini', 'gpt-5-nano']) {
      expect(MODEL_PRICING.findIndex((p) => p.match === variant)).toBeLessThan(flagshipIndex)
    }
  })
})

describe('formatUsd', () => {
  it('keeps four decimals below one cent so tiny spends are not shown as $0.00', () => {
    expect(formatUsd(0.0042)).toBe('$0.0042')
  })

  it('uses two decimals at or above one cent', () => {
    expect(formatUsd(1.2345)).toBe('$1.23')
    expect(formatUsd(0)).toBe('$0.00')
  })
})
