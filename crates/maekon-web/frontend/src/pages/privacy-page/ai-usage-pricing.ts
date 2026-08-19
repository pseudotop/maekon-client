/**
 * #9466: LOCAL reference price table for estimating today's BYOK AI spend.
 *
 * Deliberately a static, in-repo table — never a network lookup (the privacy
 * panel must not create egress to display an egress/spend surface). Prices
 * are USD per 1M tokens, indicative published list prices; the UI labels the
 * result as an estimate. Unknown models render token counts only.
 */

export interface ModelPricing {
  /** Case-insensitive substring matched against the configured model id. */
  match: string
  inputPerMillionUsd: number
  outputPerMillionUsd: number
}

/**
 * Ordered — first match wins, so EVERY model family with cheaper variants
 * must list those variants before the generic family row (mini/nano before
 * flagship), or the variant silently prices at the flagship rate.
 * Prices snapshot: 2026-07 published list prices.
 */
export const MODEL_PRICING: ModelPricing[] = [
  // Anthropic
  { match: 'claude-fable', inputPerMillionUsd: 20, outputPerMillionUsd: 100 },
  { match: 'claude-opus', inputPerMillionUsd: 15, outputPerMillionUsd: 75 },
  { match: 'claude-sonnet', inputPerMillionUsd: 3, outputPerMillionUsd: 15 },
  { match: 'claude-haiku', inputPerMillionUsd: 0.8, outputPerMillionUsd: 4 },
  // OpenAI
  { match: 'gpt-5-mini', inputPerMillionUsd: 0.25, outputPerMillionUsd: 2 },
  { match: 'gpt-5-nano', inputPerMillionUsd: 0.05, outputPerMillionUsd: 0.4 },
  { match: 'gpt-5', inputPerMillionUsd: 1.25, outputPerMillionUsd: 10 },
  { match: 'gpt-4o-mini', inputPerMillionUsd: 0.15, outputPerMillionUsd: 0.6 },
  { match: 'gpt-4o', inputPerMillionUsd: 2.5, outputPerMillionUsd: 10 },
  { match: 'gpt-4.1', inputPerMillionUsd: 2, outputPerMillionUsd: 8 },
  // Google
  { match: 'gemini-2.5-pro', inputPerMillionUsd: 1.25, outputPerMillionUsd: 10 },
  { match: 'gemini-2.5-flash', inputPerMillionUsd: 0.15, outputPerMillionUsd: 0.6 },
  { match: 'gemini', inputPerMillionUsd: 1.25, outputPerMillionUsd: 10 },
  // Local runtimes — free by definition.
  { match: 'ollama', inputPerMillionUsd: 0, outputPerMillionUsd: 0 },
]

export interface SpendEstimate {
  usd: number
  pricing: ModelPricing
}

/**
 * Estimate today's spend for `model` from the local table.
 * Returns `null` when the model is unknown (token counts still render).
 */
export function estimateSpendUsd(
  model: string | null | undefined,
  inputTokens: number,
  outputTokens: number,
): SpendEstimate | null {
  if (!model) return null
  const normalized = model.toLowerCase()
  const pricing = MODEL_PRICING.find((entry) => normalized.includes(entry.match))
  if (!pricing) return null
  const usd =
    (inputTokens / 1_000_000) * pricing.inputPerMillionUsd + (outputTokens / 1_000_000) * pricing.outputPerMillionUsd
  return { usd, pricing }
}

/** Format a USD estimate: sub-cent amounts keep 4 decimals, else 2. */
export function formatUsd(usd: number): string {
  const decimals = usd > 0 && usd < 0.01 ? 4 : 2
  return `$${usd.toFixed(decimals)}`
}
