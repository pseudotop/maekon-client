/**
 * #9466: AI usage card on the privacy/egress trust surface.
 *
 * Asserts: (a) inside the Tauri runtime the card renders token counts, a
 * local-table spend estimate and the today-only disclaimer; (b) an unknown
 * model renders the pricing-unknown copy instead of a dollar figure; (c)
 * outside the Tauri runtime the card renders nothing at all (the IPC surface
 * does not exist in the plain browser dashboard).
 */

import { screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import en from '../../../i18n/locales/en.json'
import AiUsageCard, { type TokenUsagePayload } from '../AiUsageCard'

const invokeSpy = vi.fn()
let tauriRuntime = true

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeSpy(...args),
}))

vi.mock('../../../utils/platform', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../../utils/platform')>()
  return {
    ...actual,
    isTauriRuntime: () => tauriRuntime,
  }
})

function usage(overrides: Partial<TokenUsagePayload> = {}): TokenUsagePayload {
  return {
    totalInputTokens: 2_000_000,
    totalOutputTokens: 1_000_000,
    dailyBudget: 6_000_000,
    budgetRemaining: 3_000_000,
    model: 'claude-sonnet-5',
    provider: 'anthropic',
    ...overrides,
  }
}

afterEach(() => {
  invokeSpy.mockReset()
  tauriRuntime = true
})

describe('AiUsageCard', () => {
  it('renders token counts, a local spend estimate and the disclaimer in Tauri', async () => {
    invokeSpy.mockResolvedValue(usage())
    renderWithProviders(<AiUsageCard />)

    await waitFor(() => {
      expect(screen.getByText(en.privacy.aiUsage.title)).toBeInTheDocument()
    })
    expect(invokeSpy).toHaveBeenCalledWith('get_token_usage')
    // 2M in × $3/M + 1M out × $15/M = $21.00 from the LOCAL table.
    expect(screen.getByText('$21.00')).toBeInTheDocument()
    expect(screen.getByText('claude-sonnet-5')).toBeInTheDocument()
    expect(screen.getByText(en.privacy.aiUsage.todayOnlyNote)).toBeInTheDocument()
    // 3M of 6M budget = 50%.
    expect(screen.getByText(/50%/)).toBeInTheDocument()
  })

  it('renders the pricing-unknown copy for a model outside the local table', async () => {
    invokeSpy.mockResolvedValue(usage({ model: 'mystery-llm-9000' }))
    renderWithProviders(<AiUsageCard />)

    await waitFor(() => {
      expect(screen.getByText(en.privacy.aiUsage.pricingUnknown)).toBeInTheDocument()
    })
    expect(screen.queryByText(/^\$/)).not.toBeInTheDocument()
  })

  it('renders nothing outside the Tauri runtime', async () => {
    tauriRuntime = false
    const { container } = renderWithProviders(<AiUsageCard />)
    expect(invokeSpy).not.toHaveBeenCalled()
    expect(container).toBeEmptyDOMElement()
  })
})
