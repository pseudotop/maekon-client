import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import SelfResourceSnapshot, { type ResourceUsageSnapshot } from './SelfResourceSnapshot'

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

vi.mock('../utils/platform', () => ({
  IS_TAURI: true,
}))

const healthySnapshot: ResourceUsageSnapshot = {
  generated_at: '2026-07-17T00:00:00Z',
  rss_bytes: 32 * 1024 * 1024,
  cpu_percent: 0.5,
  rss_budget_bytes: 200 * 1024 * 1024,
  cpu_budget_percent: 2,
  rss_within_budget: true,
  cpu_within_budget: true,
  measured: true,
}

describe('SelfResourceSnapshot', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  it('keeps healthy budget status neutral', async () => {
    mockInvoke.mockResolvedValue(healthySnapshot)

    renderWithProviders(<SelfResourceSnapshot />)

    const badges = await screen.findAllByText('Within budget')
    expect(badges).toHaveLength(2)
    for (const badge of badges) {
      expect(badge).toHaveClass('bg-status-disconnected/20')
      expect(badge).not.toHaveClass('bg-semantic-success/20')
    }
  })

  it('highlights only the breached metric', async () => {
    mockInvoke.mockResolvedValue({
      ...healthySnapshot,
      rss_within_budget: false,
    } satisfies ResourceUsageSnapshot)

    renderWithProviders(<SelfResourceSnapshot />)

    const warning = await screen.findByText('Over budget')
    expect(warning).toHaveClass('bg-semantic-warning/20')
    expect(screen.getByText('Within budget')).toHaveClass('bg-status-disconnected/20')
  })

  it('shows n/a without status emphasis when measurement is unavailable', async () => {
    mockInvoke.mockResolvedValue({
      ...healthySnapshot,
      measured: false,
    } satisfies ResourceUsageSnapshot)

    renderWithProviders(<SelfResourceSnapshot />)

    await waitFor(() => expect(screen.getAllByText('n/a')).toHaveLength(2))
    expect(screen.queryByText('Within budget')).not.toBeInTheDocument()
    expect(screen.queryByText('Over budget')).not.toBeInTheDocument()
  })
})
