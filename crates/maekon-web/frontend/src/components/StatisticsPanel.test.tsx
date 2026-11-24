import { render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import i18n from '../i18n'
import StatisticsPanel from './StatisticsPanel'

const statistics = {
  deep_work_hours: 2.25,
  communication_hours: 0.5,
  meeting_hours: 1,
  context_switches: 4,
  longest_focus_mins: 135,
  longest_focus_content: 'Editor',
  regime_distribution: {},
  comparison: { deep_work_delta: 0.5, communication_delta: -1.25, context_switch_delta: 2 },
}

// #13740: the day view showed deep work as "2.3h" while the Focus page showed
// the same length as "2h 15m". Both now go through the shared duration rule.
describe('StatisticsPanel durations', () => {
  afterEach(async () => {
    await i18n.changeLanguage('en')
  })

  it('shows hours and minutes with the shared rule', async () => {
    await i18n.changeLanguage('en')

    render(<StatisticsPanel statistics={statistics} />)

    expect(screen.getByText('2h 15m')).toBeInTheDocument()
    expect(screen.getByText('30m')).toBeInTheDocument()
    expect(screen.getByText('1h')).toBeInTheDocument()
    expect(screen.getByText('↑ 30m')).toBeInTheDocument()
    expect(screen.getByText('↓ 1h 15m')).toBeInTheDocument()
    expect(screen.getByText('Longest Focus: 2h 15m')).toBeInTheDocument()
    expect(screen.queryByText(/\d\.\dh/)).not.toBeInTheDocument()
  })

  it('uses Korean units on a Korean screen', async () => {
    await i18n.changeLanguage('ko')

    render(<StatisticsPanel statistics={statistics} />)

    expect(screen.getByText('2시간 15분')).toBeInTheDocument()
    expect(screen.getByText('30분')).toBeInTheDocument()
    expect(screen.getByText('1시간')).toBeInTheDocument()
    expect(screen.getByText('↓ 1시간 15분')).toBeInTheDocument()
    // #13738: the longest-focus line is a translation key, not an English label.
    expect(screen.getByText('최장 집중: 2시간 15분')).toBeInTheDocument()
    expect(screen.queryByText(/Longest Focus/)).not.toBeInTheDocument()
  })
})
