import { fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { useWbsAssignee } from '../hooks/useWbsAssignee'
import type { WbsExecution } from '../wbsTypes'
import { WbsAssigneeTooltip } from './WbsAssigneeTooltip'

vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))
const mockHook = vi.hoisted(() => vi.fn())
vi.mock('../hooks/useWbsAssignee', () => ({ useWbsAssignee: mockHook }))

let state: ReturnType<typeof useWbsAssignee>
beforeEach(() => {
  state = {
    status: 'idle',
    offer: null,
    recommendation: null,
    execution: null,
    error: null,
    requestConsent: vi.fn(),
    acceptConsent: vi.fn(),
    refresh: vi.fn(),
    choose: vi.fn(),
    cancel: vi.fn(),
    checkExecution: vi.fn(),
  }
  mockHook.mockReturnValue(state)
})

const verified: WbsExecution = {
  operation_id: 'operation-1',
  disposition: 'applied_verified',
  native_outcome: 'written_unverified',
  readback: 'matches',
  audit: 'completion_accepted',
}

function candidates() {
  state.status = 'ready'
  state.recommendation = {
    session_id: 'session-1',
    query_generation: '9007199254740993',
    context: {
      source_kind: 'local_document_roster',
      organization_id: null,
      wbs_item_id: 'task-1',
      wbs_version_id: null,
      local: {
        document_registration_id: 'synthetic-document',
        input_revision: 'input-1',
        input_hash: '1'.repeat(64),
        roster_revision: 'roster-1',
        roster_hash: '2'.repeat(64),
      },
    },
    source: {
      source_kind: 'local_document_roster',
      snapshot_id: 'synthetic-snapshot',
      snapshot_version: '1',
      snapshot_hash: '3'.repeat(64),
      wbs_version_id: null,
      wbs_content_hash: null,
      approval_id: null,
      local: {
        input: {
          document_registration_id: 'synthetic-document',
          input_revision: 'input-1',
          input_hash: '1'.repeat(64),
          roster_revision: 'roster-1',
          roster_hash: '2'.repeat(64),
        },
        provider_selection_digest: '4'.repeat(64),
        provider_name: 'Synthetic provider',
        model: 'synthetic-model',
        prompt_revision: 'prompt-1',
      },
    },
    synthetic: true,
    provenance: [],
    target_hint: { row: 5, column: 5 },
    expires_at: new Date(Date.now() + 60_000).toISOString(),
    candidates: [
      {
        candidate_id: 'eligible',
        display_name: 'Synthetic A',
        rank: 1,
        eligible: true,
        rank_reason: 'Supplied skill',
        candidate_proof: 'a'.repeat(64),
      },
      {
        candidate_id: 'ineligible',
        display_name: 'Synthetic B',
        rank: 2,
        eligible: false,
        rank_reason: 'Unavailable',
        candidate_proof: 'b'.repeat(64),
      },
    ],
  }
}
describe('WbsAssigneeTooltip', () => {
  it('requires an explicit consent request', () => {
    render(<WbsAssigneeTooltip />)
    expect(screen.getByTestId('wbs-request-consent')).toBeEnabled()
    expect(state.requestConsent).not.toHaveBeenCalled()
    fireEvent.click(screen.getByTestId('wbs-request-consent'))
    expect(state.requestConsent).toHaveBeenCalledTimes(1)
  })

  it('shows restart guidance only for the known disabled automation code', () => {
    state.status = 'unavailable'
    state.error = 'wbs_automation_disabled'
    render(<WbsAssigneeTooltip />)
    expect(screen.getByTestId('wbs-unavailable')).toHaveTextContent('wbsAssignee.automationDisabled')
  })

  it('keeps unknown failures on the generic unavailable message', () => {
    state.status = 'unavailable'
    state.error = 'wbs_unrecognized_error'
    render(<WbsAssigneeTooltip />)
    expect(screen.getByTestId('wbs-unavailable')).toHaveTextContent('wbsAssignee.unavailable')
  })

  it.each([
    ['wbs_consent_required', 'consentRequired'],
    ['wbs_consent_changed', 'consentChanged'],
    ['wbs_policy_blocked', 'policyBlocked'],
    ['wbs_sandbox_unsupported', 'sandboxUnsupported'],
    ['wbs_configuration_changed', 'configurationChanged'],
    ['wbs_expired', 'expired'],
    ['wbs_unavailable', 'requestUnavailable'],
    ['wbs_outcome_unknown', 'outcomeUnknown'],
    ['wbs_readback_unavailable', 'readbackUnavailable'],
  ])('shows distinct localized guidance for %s', (code, key) => {
    state.status = 'unavailable'
    state.error = code
    render(<WbsAssigneeTooltip />)
    expect(screen.getByTestId('wbs-unavailable')).toHaveTextContent(`wbsAssignee.${key}`)
    expect(state.requestConsent).not.toHaveBeenCalled()
    expect(state.choose).not.toHaveBeenCalled()
    expect(screen.queryByTestId('wbs-result')).not.toBeInTheDocument()
  })

  it('keeps result checking available after an uncertain write and never offers a fresh start', () => {
    candidates()
    state.execution = { ...verified, disposition: 'outcome_unknown', native_outcome: 'outcome_unknown' }
    state.error = 'wbs_outcome_unknown'
    render(<WbsAssigneeTooltip />)
    expect(screen.getByTestId('wbs-unavailable')).toHaveTextContent('wbsAssignee.outcomeUnknown')
    expect(screen.queryByTestId('wbs-request-consent')).not.toBeInTheDocument()
    expect(screen.getByTestId('wbs-candidate-eligible')).toBeDisabled()
    fireEvent.click(screen.getByTestId('wbs-check-result'))
    expect(state.checkExecution).toHaveBeenCalledTimes(1)
    expect(state.choose).not.toHaveBeenCalled()
  })

  it('allows a proof-bound eligible selection with the keyboard and disables ineligible people', async () => {
    candidates()
    const user = userEvent.setup()
    render(<WbsAssigneeTooltip />)
    const eligible = screen.getByTestId('wbs-candidate-eligible')
    eligible.focus()
    await user.keyboard('{Enter}')
    expect(state.choose).toHaveBeenCalledExactlyOnceWith(state.recommendation?.candidates[0])
    expect(screen.getByTestId('wbs-candidate-ineligible')).toBeDisabled()
    expect(screen.getByTestId('wbs-synthetic')).toBeInTheDocument()
  })

  it('suspends choices while the shared confirmation modal owns interaction', () => {
    candidates()
    render(<WbsAssigneeTooltip suspended />)
    expect(screen.getByTestId('wbs-candidate-eligible')).toBeDisabled()
    expect(screen.getByTestId('wbs-refresh')).toBeDisabled()
    expect(screen.getByTestId('wbs-consent-cancel')).toBeDisabled()
  })

  it('shows success only when native acknowledgement, readback and audit all agree', () => {
    state.execution = verified
    render(<WbsAssigneeTooltip />)
    expect(screen.getByTestId('wbs-result')).toHaveClass('text-semantic-success')
    expect(screen.queryByTestId('wbs-check-result')).not.toBeInTheDocument()
  })

  it.each<Partial<WbsExecution>>([
    { disposition: 'outcome_unknown', native_outcome: 'outcome_unknown' },
    { disposition: 'audit_completion_failed', audit: 'completion_rejected' },
    { disposition: 'readback_mismatch', readback: 'mismatch' },
    { native_outcome: 'outcome_unknown' },
    { readback: 'unavailable' },
    { audit: 'completion_rejected' },
  ])('does not turn an incomplete or conflicting result into success: %j', (delta) => {
    candidates()
    state.execution = { ...verified, ...delta }
    render(<WbsAssigneeTooltip />)
    expect(screen.getByTestId('wbs-result')).toHaveClass('text-semantic-warning')
    expect(screen.getByTestId('wbs-candidate-eligible')).toBeDisabled()
    fireEvent.click(screen.getByTestId('wbs-check-result'))
    expect(state.checkExecution).toHaveBeenCalledTimes(1)
    expect(state.choose).not.toHaveBeenCalled()
  })
})
