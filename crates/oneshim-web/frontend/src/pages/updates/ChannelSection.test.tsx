import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Outlet, Route, Routes } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import ChannelSection from './ChannelSection'
import type { UpdatesOutletContext } from './UpdatesLayout'

function renderChannelSection(overrides: Partial<UpdatesOutletContext> = {}) {
  const context: UpdatesOutletContext = {
    settings: undefined,
    status: undefined,
    currentChannel: 'stable',
    savingChannel: false,
    handleChannelChange: vi.fn(),
    actionMutation: {} as UpdatesOutletContext['actionMutation'],
    isDownloading: false,
    versionSummary: null,
    ...overrides,
  }

  renderWithProviders(
    <Routes>
      <Route path="/" element={<Outlet context={context} />}>
        <Route index element={<ChannelSection />} />
      </Route>
    </Routes>,
    { routerProps: { initialEntries: ['/'] } },
  )

  return context
}

describe('ChannelSection', () => {
  it('keeps nightly visible but unavailable in this build', async () => {
    const user = userEvent.setup()
    const handleChannelChange = vi.fn()

    renderChannelSection({ handleChannelChange })

    const nightly = screen.getByRole('button', { name: /Nightly/i })
    expect(nightly).toBeDisabled()
    expect(screen.getByText('Not available')).toBeInTheDocument()

    await user.click(nightly)

    expect(handleChannelChange).not.toHaveBeenCalled()
  })

  it('warns when an existing setting is still pinned to nightly', () => {
    renderChannelSection({ currentChannel: 'nightly' })

    expect(screen.getByText('Nightly updates are disabled in this build')).toBeInTheDocument()
    expect(screen.queryByText('Active')).not.toBeInTheDocument()
  })
})
