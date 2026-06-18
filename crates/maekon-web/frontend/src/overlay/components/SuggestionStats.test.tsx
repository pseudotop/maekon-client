import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { I18nextProvider } from 'react-i18next'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../../i18n'
import { SuggestionStats } from './SuggestionStats'

const mockInvoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

function renderStats() {
  return render(
    <I18nextProvider i18n={i18n}>
      <SuggestionStats />
    </I18nextProvider>,
  )
}

// Aligned to the Rust SuggestionStatsDto wire contract (#5699):
//   - total_shown / total_accepted / total_rejected / total_deferred (not total/accepted/…)
//   - acceptance_rate is a fraction 0..1 (not a percent integer)
//   - by_source items carry accepted/rejected counts, not a pre-computed acceptance_rate
const statsFixture = {
  total_shown: 3,
  total_accepted: 2,
  total_rejected: 1,
  total_deferred: 0,
  acceptance_rate: 0.6667,
  by_type: [],
  by_source: [],
}

describe('SuggestionStats error/retry path', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
  })

  it('IPC 실패 시 로딩 상태가 아닌 에러 메시지 + 재시도 버튼을 노출한다', async () => {
    mockInvoke.mockRejectedValue(new Error('ipc boom'))

    renderStats()

    // Error banner is shown (not hidden behind an infinite "Loading...")
    expect(await screen.findByText('Could not load stats.')).toBeInTheDocument()
    expect(screen.queryByText('Loading...')).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
  })

  it('재시도 버튼 클릭 시 IPC 를 다시 호출하고 복구된 통계를 렌더링한다', async () => {
    // First call (stats + daily) both fail -> error state
    mockInvoke.mockRejectedValueOnce(new Error('ipc boom')).mockRejectedValueOnce(new Error('ipc boom'))
    // Retry: stats succeeds + daily succeeds
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_suggestion_stats') return Promise.resolve(statsFixture)
      if (cmd === 'get_suggestion_daily_stats') return Promise.resolve([])
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })

    renderStats()

    const retry = await screen.findByRole('button', { name: 'Retry' })
    await userEvent.click(retry)

    // acceptance_rate 0.6667 -> Math.round(0.6667 * 100) = 67%
    await waitFor(() => expect(screen.getByText('67%')).toBeInTheDocument())
    expect(mockInvoke).toHaveBeenCalledWith('get_suggestion_stats')
  })

  it('daily stats 행은 date 필드로 집계되고 DayAggregate 에 날짜가 반영된다', async () => {
    // DailyStat now uses `date` / `shown` / `accepted` / `rejected` / `deferred`
    // (was `day` / `total` / `acted` — shape mismatch caused undefined day + slice crash).
    const dailyFixture = [
      { date: '2026-01-10', shown: 5, accepted: 3, rejected: 1, deferred: 0 },
      { date: '2026-01-11', shown: 2, accepted: 1, rejected: 0, deferred: 1 },
    ]
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_suggestion_stats') return Promise.resolve(statsFixture)
      if (cmd === 'get_suggestion_daily_stats') return Promise.resolve(dailyFixture)
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })

    renderStats()

    // The truncated date label (slice(5)) should be visible in the daily trends block.
    await waitFor(() => expect(screen.getByText('01-10')).toBeInTheDocument())
    expect(screen.getByText('01-11')).toBeInTheDocument()
  })
})
