import { fireEvent, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import type { AssignmentEmailDraft } from '../../../api/assignmentEmailDraft'
import { HANDOFF_ERROR_CODES } from '../../../api/osHandoff'
import AssignmentEmailDraftPage from '../AssignmentEmailDraftPage'

const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

function fixture(overrides: Partial<AssignmentEmailDraft> = {}): AssignmentEmailDraft {
  return {
    draft_id: 'emd-draft-1',
    draft_hash: 'd'.repeat(64),
    organization_id: 'org-wd-brokerage',
    recipient: {
      contact_id: 'wd-cpc-1',
      address: 'pm@yesi-soft.example',
      contact_label: 'Synthetic PM',
      counterparty_id: 'wd-cp-yesisoft',
      organization_label: 'Yesi Software',
    },
    subject: '[Synthetic] Assignment review',
    body: 'Hello,\n\nPlease review the synthetic assignment.',
    assignment_receipt_id: 'ercv-1',
    assignment_id: 'wfa-1',
    assignment_hash: 'a'.repeat(64),
    wbs_item_id: 'wbs-1',
    revision: 1,
    created_at: '2026-08-14T00:00:00Z',
    stale: false,
    stale_reason: null,
    template_id: 'assignment-counterparty-notice',
    template_version: '1.0.0',
    template_hash: 'b'.repeat(64),
    synthetic_provenance: {
      synthetic: true,
      source_kind: 'wd_brokerage_seed',
      project_id: 'wd-prj-1',
      counterparty_id: 'wd-cp-yesisoft',
      notice: 'Synthetic demo only',
    },
    ...overrides,
  }
}

function renderPage() {
  return renderWithProviders(<AssignmentEmailDraftPage />, {
    routerProps: { initialEntries: ['/assignment-email-draft?draft=emd-draft-1'] },
  })
}

describe('AssignmentEmailDraftPage (#9627)', () => {
  beforeEach(() => {
    sessionStorage.clear()
    mockInvoke.mockReset()
    mockInvoke.mockImplementation((command: string) => {
      if (command === 'load_assignment_email_draft') return Promise.resolve(fixture())
      if (command === 'open_external_target') return Promise.resolve(undefined)
      return Promise.resolve(fixture())
    })
  })

  it('load, edit, and local save never call handoff or a provider command', async () => {
    renderPage()
    await screen.findByTestId('assignment-email-editor')

    fireEvent.change(screen.getByTestId('assignment-email-body'), { target: { value: 'Edited locally' } })
    fireEvent.click(screen.getByTestId('assignment-email-save'))

    expect(screen.getByTestId('assignment-email-save-state')).toBeInTheDocument()
    const commands = mockInvoke.mock.calls.map((call) => String(call[0]))
    expect(commands).toEqual(['load_assignment_email_draft'])
    expect(commands.some((command) => /send|provider|open_external/u.test(command))).toBe(false)
  })

  it('one explicit confirmed action opens one compose window and never sends', async () => {
    renderPage()
    await screen.findByTestId('assignment-email-editor')
    fireEvent.click(screen.getByTestId('assignment-email-confirm'))
    fireEvent.click(screen.getByTestId('assignment-email-handoff'))

    await screen.findByTestId('assignment-email-handed-off')
    const handoffs = mockInvoke.mock.calls.filter((call) => call[0] === 'open_external_target')
    expect(handoffs).toHaveLength(1)
    expect(handoffs[0][1]).toMatchObject({
      url: expect.stringMatching(/^mailto:pm@yesi-soft\.example\?subject=/u),
    })
    expect(String(handoffs[0][1].url)).toContain('&body=')
    expect(screen.getByTestId('assignment-email-handoff')).toBeDisabled()
    expect(mockInvoke.mock.calls.some((call) => /send/u.test(String(call[0])))).toBe(false)
  })

  it('coalesces repeated activation while the OS handoff is in flight', async () => {
    let completeHandoff: (() => void) | undefined
    mockInvoke.mockImplementation((command: string) => {
      if (command === 'load_assignment_email_draft') return Promise.resolve(fixture())
      if (command === 'open_external_target') {
        return new Promise<void>((resolve) => {
          completeHandoff = resolve
        })
      }
      return Promise.resolve(undefined)
    })
    renderPage()
    await screen.findByTestId('assignment-email-editor')
    fireEvent.click(screen.getByTestId('assignment-email-confirm'))
    const handoff = screen.getByTestId('assignment-email-handoff')
    fireEvent.click(handoff)
    fireEvent.click(handoff)

    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter((call) => call[0] === 'open_external_target')).toHaveLength(1),
    )
    completeHandoff?.()
    await screen.findByTestId('assignment-email-handed-off')
  })

  it('supports keyboard activation and exposes labelled editor controls', async () => {
    const user = userEvent.setup()
    renderPage()
    await screen.findByTestId('assignment-email-editor')

    expect(screen.getByTestId('assignment-email-subject')).toHaveAccessibleName()
    expect(screen.getByTestId('assignment-email-body')).toHaveAccessibleName()
    await user.click(screen.getByTestId('assignment-email-confirm'))
    screen.getByTestId('assignment-email-handoff').focus()
    await user.keyboard('{Enter}')

    await screen.findByTestId('assignment-email-handed-off')
    expect(mockInvoke.mock.calls.filter((call) => call[0] === 'open_external_target')).toHaveLength(1)
  })

  it('locks a stale draft to regenerate or cancel only', async () => {
    mockInvoke.mockImplementation((command: string) => {
      if (command === 'load_assignment_email_draft') {
        return Promise.resolve(fixture({ stale: true, stale_reason: 'counterparty_relation_changed' }))
      }
      return Promise.resolve(fixture({ draft_id: 'emd-draft-2', revision: 2 }))
    })
    renderPage()
    await screen.findByTestId('assignment-email-stale')

    expect(screen.getByTestId('assignment-email-subject')).toBeDisabled()
    expect(screen.getByTestId('assignment-email-body')).toBeDisabled()
    expect(screen.getByTestId('assignment-email-save')).toBeDisabled()
    expect(screen.getByTestId('assignment-email-handoff')).toBeDisabled()
    expect(screen.getByTestId('assignment-email-regenerate')).toBeEnabled()
    expect(screen.getByTestId('assignment-email-cancel-stale')).toBeEnabled()
  })

  it('rejects subject header injection before OS handoff', async () => {
    renderPage()
    await screen.findByTestId('assignment-email-editor')
    fireEvent.change(screen.getByTestId('assignment-email-subject'), {
      target: { value: 'Review\r\nBcc: victim@gmail.com' },
    })

    expect(screen.getByTestId('assignment-email-handoff')).toBeDisabled()
    expect(mockInvoke.mock.calls.filter((call) => call[0] === 'open_external_target')).toHaveLength(0)
  })

  it('distinguishes no mail app and focuses the recovery message', async () => {
    mockInvoke.mockImplementation((command: string) => {
      if (command === 'load_assignment_email_draft') return Promise.resolve(fixture())
      if (command === 'open_external_target') {
        return Promise.reject({ code: HANDOFF_ERROR_CODES.noHandler, message: 'no handler' })
      }
      return Promise.resolve(undefined)
    })
    renderPage()
    await screen.findByTestId('assignment-email-editor')
    fireEvent.click(screen.getByTestId('assignment-email-confirm'))
    fireEvent.click(screen.getByTestId('assignment-email-handoff'))

    const alert = await screen.findByTestId('assignment-email-error-noHandler')
    await waitFor(() => expect(alert).toHaveFocus())
    expect(screen.getByTestId('assignment-email-handoff')).toBeEnabled()
  })
})
