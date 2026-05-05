import { fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { fetchExecutionPolicies } from '../../api/client'
import Policies from './index'

vi.mock('../../api/client', () => ({
  createExecutionPolicy: vi.fn(),
  deleteExecutionPolicy: vi.fn(),
  fetchExecutionPolicies: vi.fn(),
  updateExecutionPolicy: vi.fn(),
}))

describe('Execution Policies page', () => {
  beforeEach(() => {
    vi.mocked(fetchExecutionPolicies).mockResolvedValue([])
  })

  it('explains the first useful policy instead of only showing a sparse empty state', async () => {
    renderWithProviders(<Policies />)

    await waitFor(() => {
      expect(screen.getByText('No execution policies')).toBeInTheDocument()
    })

    expect(screen.getByText('Start with one trusted local process')).toBeInTheDocument()
    expect(screen.getByText('Keep confirmation on')).toBeInTheDocument()
    expect(screen.getByText('Review the first run')).toBeInTheDocument()
  })

  it('shows a policy preview when creating a policy', async () => {
    renderWithProviders(<Policies />)

    fireEvent.click(await screen.findByRole('button', { name: 'Add Policy' }))

    expect(screen.getByText('Policy preview')).toBeInTheDocument()
    expect(screen.getByText('Confirmation required')).toBeInTheDocument()
    expect(screen.getByText('Arguments: none yet')).toBeInTheDocument()
    expect(screen.queryByText('Configured Policies (0)')).not.toBeInTheDocument()
  })
})
