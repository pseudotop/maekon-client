import { screen } from '@testing-library/react'
import { Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import type { UpdatePhase, UpdateStatus } from '../../api/client'
import i18n from '../../i18n'
import StatusSection from './StatusSection'
import UpdatesLayout from './UpdatesLayout'

const mockFetchUpdateStatus = vi.fn()

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>()
  return {
    ...actual,
    fetchSettings: vi.fn(() => new Promise(() => {})),
    fetchUpdateStatus: () => mockFetchUpdateStatus(),
    postUpdateAction: vi.fn(),
    updateSettings: vi.fn(),
  }
})

vi.mock('../../hooks/useUpdateStream', () => ({
  useUpdateStream: () => ({
    status: 'connected',
    latest: undefined,
    recoveredAt: null,
    lastError: null,
    retryCount: 0,
  }),
}))

function status(phase: UpdatePhase): UpdateStatus {
  return {
    enabled: true,
    auto_install: false,
    phase,
    message: 'Already on latest version',
    pending: null,
    download_progress: null,
    rollback: null,
    revision: 1,
    updated_at: new Date().toISOString(),
  }
}

// Selectors come from the locale resources so a copy change does not break the count.
const checkNow = () => i18n.t('updates.checkNow')
const stage = (stateKey: string) => i18n.t('updates.phaseBadge', { state: i18n.t(stateKey) })

function renderStatusPage() {
  renderWithProviders(
    <Routes>
      <Route path="/updates" element={<UpdatesLayout />}>
        <Route path="status" element={<StatusSection />} />
      </Route>
    </Routes>,
    { routerProps: { initialEntries: ['/updates/status'] } },
  )
}

// #13740: the header and the status panel each drew "Check now" on this page.
describe('Updates page', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    mockFetchUpdateStatus.mockReset()
  })

  it('offers a single Check now', async () => {
    mockFetchUpdateStatus.mockResolvedValue(status('Idle'))

    renderStatusPage()

    await screen.findByText(stage('updates.idle'))
    expect(screen.getAllByRole('button', { name: checkNow() })).toHaveLength(1)
  })

  it('disables Check now while an update is downloading', async () => {
    mockFetchUpdateStatus.mockResolvedValue(status('Downloading'))

    renderStatusPage()

    await screen.findByText(stage('updates.downloading'))
    expect(screen.getByRole('button', { name: checkNow() })).toBeDisabled()
  })
})
