/**
 * Egress transparency browser panel test (T1.2, #7910).
 *
 * Drives the real `EgressLedgerSection` with `fetchEgressLedger` stubbed, and
 * asserts: (a) ledger rows render with humanized bytes + verbatim mono
 * destination strings, (b) a `capture_blocked` row renders distinctly — the
 * "capture blocked (excluded app)" badge copy and an em-dash for bytes so
 * byte 0 never reads as "0 bytes uploaded", and (c) the range-empty state copy.
 */

import { screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import type { EgressLedgerResponse } from '../../../api/contracts'
import en from '../../../i18n/locales/en.json'
import EgressLedgerSection from '../EgressLedgerSection'

const fetchEgressLedgerSpy = vi.fn()

vi.mock('../../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../../api/client')>()
  return {
    ...actual,
    fetchEgressLedger: (...args: unknown[]) => fetchEgressLedgerSpy(...args),
  }
})

const SAMPLE: EgressLedgerResponse = {
  entries: [
    {
      record_id: 'cap-1',
      event_type: 'Context',
      event_id: null,
      byte_count: 0,
      recipient_count: 0,
      destination: 'local.capture',
      disposition: 'capture_blocked',
      consent_state: 'telemetry=true',
      occurred_at: '2026-07-07T09:00:00Z',
    },
    {
      record_id: 'up-1',
      event_type: 'Context',
      event_id: 'evt-1',
      byte_count: 2048,
      recipient_count: 1,
      destination: 'server.batch_upload',
      disposition: 'uploaded',
      consent_state: 'telemetry=true',
      occurred_at: '2026-07-06T09:00:00Z',
    },
  ],
}

afterEach(() => {
  fetchEgressLedgerSpy.mockReset()
})

describe('EgressLedgerSection', () => {
  it('renders ledger rows including a capture_blocked row rendered distinctly', async () => {
    fetchEgressLedgerSpy.mockResolvedValue(SAMPLE)

    renderWithProviders(<EgressLedgerSection />)

    // Uploaded row: humanized byte count.
    expect(await screen.findByText('2.0KB')).toBeInTheDocument()
    // capture_blocked row: the distinct "excluded app" badge copy (NOT the
    // filter-option "Capture blocked" label).
    expect(screen.getByText(en.privacy.egress.badge.capture_blocked)).toBeInTheDocument()
    // Raw destination sink strings shown verbatim (mono).
    expect(screen.getByText('local.capture')).toBeInTheDocument()
    expect(screen.getByText('server.batch_upload')).toBeInTheDocument()
    // capture_blocked byte 0 is rendered as an em-dash, not "0B".
    expect(screen.getAllByText('—').length).toBeGreaterThanOrEqual(2)
    expect(screen.queryByText('0B')).not.toBeInTheDocument()
  })

  it('shows the range-empty state when there are no entries in the selected range', async () => {
    fetchEgressLedgerSpy.mockResolvedValue({ entries: [] })

    renderWithProviders(<EgressLedgerSection />)

    // Default range preset is 7d (not "all"), so this is the in-range empty
    // copy rather than the "nothing has ever left this device" ledger-empty copy.
    expect(await screen.findByText(en.privacy.egress.empty.rangeTitle)).toBeInTheDocument()
  })
})
