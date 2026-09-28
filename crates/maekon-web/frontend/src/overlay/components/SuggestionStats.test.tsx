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
  latest_local_analysis: null,
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

  it('제안 기록이 없어도 최신 로컬 분석의 no-candidate 상태와 provenance를 표시한다', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_suggestion_stats') {
        return Promise.resolve({
          ...statsFixture,
          total_shown: 0,
          total_accepted: 0,
          total_rejected: 0,
          acceptance_rate: 0,
          latest_local_analysis: {
            status: 'no_candidate',
            reason: 'no_valid_candidate',
            producer: 'app_switch',
            source: 'llm_local',
            observed_at: '2026-09-02T08:30:00Z',
            candidate_count: 0,
            queue_count: 0,
            missing_permissions: [],
          },
        })
      }
      if (cmd === 'get_suggestion_daily_stats') return Promise.resolve([])
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })

    renderStats()

    expect(await screen.findByText('No valid local candidate found')).toBeInTheDocument()
    expect(screen.getByTestId('local-analysis-status')).toHaveAttribute('data-status', 'no_candidate')
    expect(screen.getByText(/Local.*app-switch analysis/)).toBeInTheDocument()
    expect(screen.getByText('No data yet')).toBeInTheDocument()
  })

  it('renders analysis-disabled as the exact policy blocker', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_suggestion_stats') {
        return Promise.resolve({
          ...statsFixture,
          total_shown: 0,
          latest_local_analysis: {
            status: 'policy_blocked',
            reason: 'analysis_disabled',
            producer: 'periodic',
            source: 'llm_local',
            observed_at: '2026-09-02T08:30:00Z',
            candidate_count: 0,
            queue_count: 0,
            missing_permissions: [],
          },
        })
      }
      if (cmd === 'get_suggestion_daily_stats') return Promise.resolve([])
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })

    renderStats()

    expect(await screen.findByText('Local activity suggestion generation is disabled')).toBeInTheDocument()
    expect(screen.queryByText('Local analysis is blocked by capture policy')).not.toBeInTheDocument()
  })
})
