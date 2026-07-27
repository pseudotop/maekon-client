import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { fetchAuditExport } from '../../api/client'
import EntriesSection, { auditCorrelationLabel } from './EntriesSection'

vi.mock('../../api/client', () => ({
  fetchAuditExport: vi.fn(),
}))

const sensitiveDetails = JSON.stringify({
  consent_id: 'consent-sensitive-value',
  permissions: { screen_capture: true, unredacted_external_ocr: true },
})

describe('Audit EntriesSection', () => {
  beforeEach(() => {
    // #8114: the list is backed by the DURABLE audit log (/audit/export),
    // whose privacy-bounded rows never carry a details payload.
    vi.mocked(fetchAuditExport).mockResolvedValue([
      {
        entry_id: 'entry-sensitive-value',
        timestamp: '2026-07-13T08:00:00Z',
        command_id: 'command-sensitive-value',
        action_type: 'consent_revoked',
        status: 'Completed',
        execution_time_ms: null,
      },
    ])
  })

  it('replaces raw details and identifiers with an opaque bounded correlation', async () => {
    renderWithProviders(<EntriesSection />)

    await waitFor(() => {
      expect(screen.getByText('consent_revoked')).toBeInTheDocument()
    })

    expect(
      screen.getByText(auditCorrelationLabel({ command_id: 'command-sensitive-value', entry_id: '' })),
    ).toBeInTheDocument()
    expect(screen.queryByText(sensitiveDetails)).not.toBeInTheDocument()
    expect(document.body.textContent).not.toContain('consent-sensitive-value')
    expect(document.body.textContent).not.toContain('screen_capture')
    expect(document.body.innerHTML).not.toContain('command-sensitive-value')
    expect(document.body.innerHTML).not.toContain('entry-sensitive-value')
  })

  it('falls back to the entry id when the command id is empty without exposing either value', () => {
    const first = auditCorrelationLabel({ command_id: '', entry_id: 'entry-one-sensitive' })
    const second = auditCorrelationLabel({ command_id: '', entry_id: 'entry-two-sensitive' })

    expect(first).toMatch(/^audit-[0-9a-f]{8}$/)
    expect(first).not.toContain('entry-one-sensitive')
    expect(first).not.toBe(second)
  })
})
