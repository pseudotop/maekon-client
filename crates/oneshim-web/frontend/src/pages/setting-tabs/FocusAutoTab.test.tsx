import { screen, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import type { AppSettings } from '../../api/contracts'
import FocusAutoTab from './FocusAutoTab'
import { makeDefaultFormData } from './stories-utils'

const mockUseSettingsFormContext = vi.hoisted(() => vi.fn())

vi.mock('../settings/SettingsFormContext', () => ({
  useSettingsFormContext: mockUseSettingsFormContext,
}))

describe('FocusAutoTab', () => {
  const handleRootChange = vi.fn()

  beforeEach(() => {
    handleRootChange.mockReset()
    mockUseSettingsFormContext.mockReset()
  })

  it('renders defaults when persisted settings do not include focus_auto', () => {
    const legacySettings = makeDefaultFormData()
    delete (legacySettings as Partial<AppSettings>).focus_auto

    mockUseSettingsFormContext.mockReturnValue({
      form: {
        formData: legacySettings,
        handleRootChange,
      },
    })

    renderWithProviders(<FocusAutoTab />)

    expect(screen.getByRole('checkbox', { name: /enable auto-switch/i })).not.toBeChecked()
    expect(screen.getByLabelText('Focus duration')).toHaveValue('25')
    expect(screen.getByLabelText('Cooldown')).toHaveValue('5')
    expect(screen.getByRole('heading', { name: 'Trigger Apps' })).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Schedules' })).toBeInTheDocument()

    const preview = within(screen.getByRole('complementary', { name: 'Rule preview' }))
    expect(preview.getByText('Inactive')).toBeInTheDocument()
    expect(preview.getByText('No trigger apps')).toBeInTheDocument()
    expect(preview.getByText('No schedules')).toBeInTheDocument()
  })

  it('summarizes the effective focus auto rule from saved settings', () => {
    const settings = makeDefaultFormData({
      focus_auto: {
        enabled: true,
        duration_minutes: 45,
        cooldown_secs: 60,
        trigger_apps: ['Visual Studio Code'],
        trigger_schedules: [{ start: '09:00', end: '12:00', days: ['Mon', 'Tue', 'Wed', 'Thu', 'Fri'] }],
      },
    })

    mockUseSettingsFormContext.mockReturnValue({
      form: {
        formData: settings,
        handleRootChange,
      },
    })

    renderWithProviders(<FocusAutoTab />)

    const preview = within(screen.getByRole('complementary', { name: 'Rule preview' }))
    expect(preview.getByText('Active')).toBeInTheDocument()
    expect(preview.getByText('45 min')).toBeInTheDocument()
    expect(preview.getByText('1 min')).toBeInTheDocument()
    expect(preview.getByText('Visual Studio Code')).toBeInTheDocument()
    expect(preview.getByText('09:00-12:00, Weekdays')).toBeInTheDocument()
  })
})
