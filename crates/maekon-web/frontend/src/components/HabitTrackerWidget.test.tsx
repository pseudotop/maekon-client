import { render } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { HabitStreak } from '../api/coaching'
import i18n from '../i18n'
import HabitTrackerWidget from './HabitTrackerWidget'

const mockUseHabitStreaks = vi.fn()

vi.mock('../hooks/useCoaching', () => ({
  useHabitStreaks: () => mockUseHabitStreaks(),
}))

function todayKey(): string {
  const now = new Date()
  const month = `${now.getMonth() + 1}`.padStart(2, '0')
  const day = `${now.getDate()}`.padStart(2, '0')
  return `${now.getFullYear()}-${month}-${day}`
}

function streak(minutesLogged: number, targetMinutes: number): HabitStreak {
  return {
    regime_label: 'Deep work',
    date: todayKey(),
    minutes_logged: minutesLogged,
    target_minutes: targetMinutes,
    met: minutesLogged >= targetMinutes,
  }
}

// #13740: the cell tooltip spelled progress as "30/60m" with a fixed English unit.
describe('HabitTrackerWidget progress tooltip', () => {
  afterEach(async () => {
    await i18n.changeLanguage('en')
  })

  it('states logged and target time with the shared duration format', async () => {
    await i18n.changeLanguage('en')
    mockUseHabitStreaks.mockReturnValue({ data: [streak(30, 60)], isLoading: false })

    const { container } = render(<HabitTrackerWidget />)

    expect(container.querySelector('[title="30m/1h (50%)"]')).not.toBeNull()
  })

  it('uses Korean units on a Korean screen', async () => {
    await i18n.changeLanguage('ko')
    mockUseHabitStreaks.mockReturnValue({ data: [streak(90, 120)], isLoading: false })

    const { container } = render(<HabitTrackerWidget />)

    expect(container.querySelector('[title="1시간 30분/2시간 (75%)"]')).not.toBeNull()
  })
})
