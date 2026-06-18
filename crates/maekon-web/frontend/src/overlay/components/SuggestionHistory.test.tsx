import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { I18nextProvider } from 'react-i18next'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../../i18n'
import { SuggestionHistory } from './SuggestionHistory'

const mockInvoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

function renderHistory() {
  return render(
    <I18nextProvider i18n={i18n}>
      <SuggestionHistory />
    </I18nextProvider>,
  )
}

describe('SuggestionHistory error/retry path', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
  })

  it('IPC 실패 시 빈 상태가 아닌 에러 메시지 + 재시도 버튼을 노출한다', async () => {
    mockInvoke.mockRejectedValueOnce(new Error('ipc boom'))

    renderHistory()

    // shows the error banner (does not hide it behind an empty "No history yet")
    expect(await screen.findByText('Could not load history.')).toBeInTheDocument()
    expect(screen.queryByText('No history yet')).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
  })

  it('재시도 버튼 클릭 시 IPC 를 다시 호출하고 복구된 데이터를 렌더링한다', async () => {
    mockInvoke
      .mockRejectedValueOnce(new Error('ipc boom'))
      .mockResolvedValueOnce([{ id: 'h1', title: 'Recovered entry', body: 'body', feedback: 'accepted' }])

    renderHistory()

    const retry = await screen.findByRole('button', { name: 'Retry' })
    await userEvent.click(retry)

    await waitFor(() => expect(screen.getByText('Recovered entry')).toBeInTheDocument())
    expect(mockInvoke).toHaveBeenCalledTimes(2)
    expect(mockInvoke).toHaveBeenCalledWith('get_suggestion_history', { limit: 50 })
  })
})
