import { screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import ScheduleSettings from './ScheduleSettings'
import { makeDefaultFormData } from './stories-utils'

// #7678: `power_status_available` PLATFORM-capability regression coverage.
// Windows/Linux never get real battery/power data (`PowerStatus::default()`),
// so "Pause on Battery Saver" must be honestly disabled there instead of
// silently accepting a toggle that can never fire.
describe('ScheduleSettings', () => {
  it('keeps the battery-saver toggle enabled when powerStatusAvailable is true (default)', () => {
    const formData = makeDefaultFormData()

    renderWithProviders(<ScheduleSettings schedule={formData.schedule} onChange={vi.fn()} />)

    expect(screen.getByRole('checkbox', { name: /Pause on Battery Saver/i })).not.toBeDisabled()
    expect(screen.queryByText(/Not available on this platform/i)).not.toBeInTheDocument()
  })

  it('#7678 disables the battery-saver toggle and shows a not-available notice when powerStatusAvailable=false', () => {
    const formData = makeDefaultFormData()

    renderWithProviders(
      <ScheduleSettings schedule={formData.schedule} onChange={vi.fn()} powerStatusAvailable={false} />,
    )

    expect(screen.getByRole('checkbox', { name: /Pause on Battery Saver/i })).toBeDisabled()
    expect(screen.getByText(/Not available on this platform/i)).toBeInTheDocument()
  })
})
