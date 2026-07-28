/**
 * Memory-graph claims browser panel test (T1.3, #7911).
 *
 * Drives the real `ClaimsSection` with `fetchClaims` / `retractClaim` stubbed,
 * and asserts: (a) claim rows render with the belief text, verbatim mono source
 * and evidence count, (b) the empty state renders when there are no claims, and
 * (c) the one-click Retract action opens a destructive confirm dialog and, on
 * confirm, calls `retractClaim` with the claim id (the user-retraction flow).
 */

import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import type { ClaimListResponse } from '../../../api/contracts'
import en from '../../../i18n/locales/en.json'
import ClaimsSection from '../ClaimsSection'

const fetchClaimsSpy = vi.fn()
const retractClaimSpy = vi.fn()

vi.mock('../../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../../api/client')>()
  return {
    ...actual,
    fetchClaims: (...args: unknown[]) => fetchClaimsSpy(...args),
    retractClaim: (...args: unknown[]) => retractClaimSpy(...args),
  }
})

const SAMPLE: ClaimListResponse = {
  total: 1,
  claims: [
    {
      claim_id: 'clm_active',
      kind: 'reflective',
      text: 'morning deep-work blocks',
      source: 'digest_highlight',
      confidence: 0.82,
      status: 'active',
      created_at: 1_700_000_000,
      updated_at: 1_700_000_500,
      evidence_count: 2,
      evidence_segment_ids: ['seg_a', 'seg_b'],
      supersedes_claim_ids: [],
    },
  ],
}

afterEach(() => {
  fetchClaimsSpy.mockReset()
  retractClaimSpy.mockReset()
})

describe('ClaimsSection', () => {
  it('renders claim rows with belief text, mono source and evidence count', async () => {
    fetchClaimsSpy.mockResolvedValue(SAMPLE)

    renderWithProviders(<ClaimsSection />)

    expect(await screen.findByText('morning deep-work blocks')).toBeInTheDocument()
    // Raw provenance source shown verbatim.
    expect(screen.getByText('digest_highlight')).toBeInTheDocument()
    // Evidence count from the edge summary.
    expect(screen.getByText('2')).toBeInTheDocument()
    // An active claim exposes the Retract action.
    expect(screen.getByRole('button', { name: en.privacy.claims.retract.button })).toBeInTheDocument()
  })

  it('keeps compact metadata and action columns on one line', async () => {
    fetchClaimsSpy.mockResolvedValue(SAMPLE)

    renderWithProviders(<ClaimsSection />)

    await screen.findByText('morning deep-work blocks')
    expect(screen.getByRole('table')).toHaveClass('min-w-[52rem]')
    expect(screen.getByRole('columnheader', { name: en.privacy.claims.col.kind })).toHaveClass('whitespace-nowrap')
    expect(screen.getByRole('columnheader', { name: en.privacy.claims.col.status })).toHaveClass('whitespace-nowrap')
    expect(screen.getByText('digest_highlight').closest('td')).toHaveClass('whitespace-nowrap')
    expect(screen.getByRole('button', { name: en.privacy.claims.retract.button }).closest('td')).toHaveClass(
      'whitespace-nowrap',
    )
  })

  it('shows the empty state when there are no claims', async () => {
    fetchClaimsSpy.mockResolvedValue({ claims: [], total: 0 })

    renderWithProviders(<ClaimsSection />)

    // Default filters (30d range) are active, so this is the filtered-empty copy.
    expect(await screen.findByText(en.privacy.claims.empty.filteredTitle)).toBeInTheDocument()
  })

  it('retracts a claim through a destructive confirm dialog', async () => {
    const user = userEvent.setup()
    fetchClaimsSpy.mockResolvedValue(SAMPLE)
    retractClaimSpy.mockResolvedValue({
      claim: { ...SAMPLE.claims[0], status: 'retracted' },
      already_retracted: false,
    })

    renderWithProviders(<ClaimsSection />)

    await screen.findByText('morning deep-work blocks')
    await user.click(screen.getByRole('button', { name: en.privacy.claims.retract.button }))

    // The confirm dialog appears; retract has NOT fired yet.
    expect(await screen.findByText(en.privacy.claims.retract.confirmTitle)).toBeInTheDocument()
    expect(retractClaimSpy).not.toHaveBeenCalled()

    await user.click(screen.getByRole('button', { name: en.privacy.claims.retract.confirmButton }))

    expect(retractClaimSpy).toHaveBeenCalledWith('clm_active')
  })
})
