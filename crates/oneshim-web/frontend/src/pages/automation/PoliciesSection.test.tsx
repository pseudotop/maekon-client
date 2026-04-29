import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { fetchAutomationContracts, fetchPolicies } from '../../api/client'
import type { AutomationContext } from './AutomationLayout'
import PoliciesSection from './PoliciesSection'

const mockUseTypedOutletContext = vi.hoisted(() => vi.fn())

vi.mock('../../routes', () => ({
  useTypedOutletContext: mockUseTypedOutletContext,
}))

vi.mock('../../api/client', () => ({
  fetchAutomationContracts: vi.fn(),
  fetchPolicies: vi.fn(),
}))

describe('Automation PoliciesSection', () => {
  beforeEach(() => {
    mockUseTypedOutletContext.mockReset()
    vi.mocked(fetchPolicies).mockResolvedValue({
      automation_enabled: false,
      sandbox_profile: 'Standard',
      sandbox_enabled: true,
      allow_network: false,
      external_data_policy: 'strict',
      scene_action_override_enabled: false,
      scene_action_override_active: false,
      scene_action_override_expires_at: null,
      scene_action_override_issue: null,
    })
    vi.mocked(fetchAutomationContracts).mockResolvedValue({
      scene_schema_version: '1',
      audit_schema_version: '1',
      scene_action_schema_version: '1',
    })
  })

  it('guides the next safe automation setup step when automation is idle', async () => {
    mockUseTypedOutletContext.mockReturnValue({
      status: { enabled: false },
      stats: { total_executions: 0 },
    } satisfies Partial<AutomationContext>)

    renderWithProviders(<PoliciesSection />)

    await waitFor(() => {
      expect(screen.getByText('Automation Inactive')).toBeInTheDocument()
    })

    expect(screen.getByText('Enable automation in Settings')).toBeInTheDocument()
    expect(screen.getByText('Keep policies explicit')).toBeInTheDocument()
    expect(screen.getByText('Audit the first run')).toBeInTheDocument()
  })
})
