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
      // #9495: must be a token the server actually EMITS on the response path
      // (the PascalCase enum: PiiFilterStrict | PiiFilterStandard |
      // AllowFiltered). The previous 'strict' matched no serde variant at all;
      // note that lowercase aliases like 'disabled' DO exist — but only as
      // request-side legacy aliases (settings_validation.rs), never in
      // responses, so response fixtures must not use them.
      external_data_policy: 'PiiFilterStrict',
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

  it('renders the wire external_data_policy token on the active policy card (#9495)', async () => {
    // Active automation → the policy grid renders, consuming the fixture
    // token verbatim. This is the assertion that makes an invalid fixture
    // token (like the old 'strict') fail instead of silently passing.
    mockUseTypedOutletContext.mockReturnValue({
      status: { enabled: true },
      stats: { total_executions: 3 },
    } satisfies Partial<AutomationContext>)

    renderWithProviders(<PoliciesSection />)

    await waitFor(() => {
      expect(screen.getByText('PiiFilterStrict')).toBeInTheDocument()
    })
  })
})
