import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { fetchAuditLogs } from '../../api/client'
import HistorySection from './HistorySection'

vi.mock('../../api/client', () => ({
  fetchAuditLogs: vi.fn(),
}))

describe('Automation HistorySection', () => {
  beforeEach(() => {
    vi.mocked(fetchAuditLogs).mockResolvedValue([])
  })

  it('explains why execution history is empty and where entries come from', async () => {
    renderWithProviders(<HistorySection />)

    await waitFor(() => {
      expect(screen.getByText('No Audit Entries')).toBeInTheDocument()
    })

    expect(screen.getByText('Run an approved command')).toBeInTheDocument()
    expect(screen.getByText('Use status filters')).toBeInTheDocument()
    expect(screen.getByText('Check elapsed time')).toBeInTheDocument()
  })
})
