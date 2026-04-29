import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import { type Tab, Tabs } from '../Tabs'

const tabs: Tab[] = [
  { id: 'overview', label: 'Overview' },
  { id: 'privacy', label: 'Privacy', disabled: true },
  { id: 'automation', label: 'Automation' },
]

function TabsHarness() {
  const [activeTab, setActiveTab] = useState('overview')

  return <Tabs tabs={tabs} activeTab={activeTab} onTabChange={setActiveTab} ariaLabel="Settings sections" />
}

describe('Tabs', () => {
  it('changes tabs when clicked', async () => {
    const user = userEvent.setup()
    const onTabChange = vi.fn()

    renderWithProviders(
      <Tabs tabs={tabs} activeTab="overview" onTabChange={onTabChange} ariaLabel="Settings sections" />,
    )

    await user.click(screen.getByRole('tab', { name: 'Automation' }))

    expect(onTabChange).toHaveBeenCalledWith('automation')
  })

  it('supports keyboard navigation and skips disabled tabs', async () => {
    const user = userEvent.setup()

    renderWithProviders(<TabsHarness />)

    const overviewTab = screen.getByRole('tab', { name: 'Overview' })
    overviewTab.focus()

    await user.keyboard('{ArrowRight}')

    const automationTab = screen.getByRole('tab', { name: 'Automation' })
    expect(automationTab).toHaveAttribute('aria-selected', 'true')
    expect(automationTab).toHaveFocus()

    await user.keyboard('{ArrowLeft}')

    expect(overviewTab).toHaveAttribute('aria-selected', 'true')
    expect(overviewTab).toHaveFocus()
  })

  it('falls back to the first enabled tab when the active tab is unavailable', () => {
    renderWithProviders(<Tabs tabs={tabs} activeTab="missing" onTabChange={vi.fn()} ariaLabel="Settings sections" />)

    const overviewTab = screen.getByRole('tab', { name: 'Overview' })
    const automationTab = screen.getByRole('tab', { name: 'Automation' })

    expect(overviewTab).toHaveAttribute('aria-selected', 'true')
    expect(overviewTab).toHaveAttribute('tabindex', '0')
    expect(automationTab).toHaveAttribute('aria-selected', 'false')
  })

  it('links tabs to tab panels when an id base is provided', () => {
    renderWithProviders(
      <Tabs tabs={tabs} activeTab="overview" onTabChange={vi.fn()} ariaLabel="Settings sections" idBase="settings" />,
    )

    expect(screen.getByRole('tab', { name: 'Overview' })).toHaveAttribute('id', 'settings-tab-overview')
    expect(screen.getByRole('tab', { name: 'Overview' })).toHaveAttribute('aria-controls', 'settings-panel-overview')
  })
})
