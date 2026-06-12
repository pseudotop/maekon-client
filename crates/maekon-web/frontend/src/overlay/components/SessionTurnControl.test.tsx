import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { I18nextProvider } from 'react-i18next'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../../i18n'
import { SessionTurnControl } from './SessionTurnControl'

// IPC invoke mock — intercept interrupt/steer commands and assert the args.
const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

function renderControl(sessionId = 'thr_42') {
  return render(
    <I18nextProvider i18n={i18n}>
      <SessionTurnControl sessionId={sessionId} />
    </I18nextProvider>,
  )
}

describe('SessionTurnControl (E21 #5017 live consumer)', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockInvoke.mockResolvedValue(undefined)
  })

  it('Interrupt invokes interrupt_session_turn with the session id', async () => {
    renderControl('thr_42')
    fireEvent.click(screen.getByRole('button', { name: 'Interrupt' }))
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('interrupt_session_turn', { sessionId: 'thr_42' })
    })
  })

  it('Send invokes steer_session_turn with the session id and message', async () => {
    renderControl('thr_42')
    fireEvent.change(screen.getByPlaceholderText('Steer the turn…'), {
      target: { value: 'go left' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('steer_session_turn', {
        sessionId: 'thr_42',
        message: 'go left',
      })
    })
  })

  it('does not steer an empty message', async () => {
    renderControl('thr_42')
    const send = screen.getByRole('button', { name: 'Send' })
    expect(send).toBeDisabled()
    fireEvent.click(send)
    expect(mockInvoke).not.toHaveBeenCalledWith('steer_session_turn', expect.anything())
  })
})
