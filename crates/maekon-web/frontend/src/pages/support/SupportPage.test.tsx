import { fireEvent, screen, within } from '@testing-library/react'
import { Route, Routes } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import SupportPage from './SupportPage'

// The diagnostics tools have their own tests; this one covers the page shell.
vi.mock('../../components/SupportToolsCard', () => ({ default: () => <div data-testid="support-tools" /> }))

describe('SupportPage', () => {
  it('links to System Metrics, which left the Dashboard first screen (#13735)', () => {
    renderWithProviders(
      <Routes>
        <Route path="/support" element={<SupportPage />} />
        <Route path="/monitoring" element={<div data-testid="monitoring-screen" />} />
      </Routes>,
      { routerProps: { initialEntries: ['/support'] } },
    )

    expect(screen.getByTestId('support-tools')).toBeInTheDocument()
    fireEvent.click(within(screen.getByTestId('support-system-metrics')).getByRole('button'))
    expect(screen.getByTestId('monitoring-screen')).toBeInTheDocument()
  })
})
