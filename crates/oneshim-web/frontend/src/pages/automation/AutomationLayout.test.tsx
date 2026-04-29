import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { fetchAutomationStats, fetchAutomationStatus } from '../../api/client'
import AutomationLayout from './AutomationLayout'

vi.mock('../../api/client', () => ({
  fetchAutomationStats: vi.fn(),
  fetchAutomationStatus: vi.fn(),
}))

describe('AutomationLayout', () => {
  beforeEach(() => {
    vi.mocked(fetchAutomationStatus).mockResolvedValue({
      enabled: false,
      sandbox_enabled: true,
      sandbox_profile: 'Standard',
      ocr_provider: 'Local',
      llm_provider: 'Local',
      ocr_source: 'local',
      llm_source: 'local',
      ocr_fallback_reason: null,
      llm_fallback_reason: null,
      external_data_policy: 'strict',
      pending_audit_entries: 0,
    })
    vi.mocked(fetchAutomationStats).mockResolvedValue({
      total_executions: 0,
      successful: 0,
      failed: 0,
      denied: 0,
      timeout: 0,
      avg_elapsed_ms: 0,
      success_rate: 0,
      blocked_rate: 0,
      p95_elapsed_ms: 0,
      timing_samples: 0,
    })
  })

  it('renders idle automation metrics with neutral visual weight', async () => {
    renderWithProviders(<AutomationLayout />)

    await waitFor(() => {
      expect(screen.getByText('Execution Statistics')).toBeInTheDocument()
    })

    expect(screen.getByText('Disabled')).not.toHaveClass('text-semantic-error')
    expect(screen.getByLabelText('Successful: 0')).not.toHaveClass('text-semantic-success')
    expect(screen.getByLabelText('Failed: 0')).not.toHaveClass('text-semantic-error')
    expect(screen.getByLabelText('Denied: 0')).not.toHaveClass('text-semantic-warning')
    expect(screen.getByLabelText('Timeout: 0')).not.toHaveClass('text-semantic-warning')
  })
})
