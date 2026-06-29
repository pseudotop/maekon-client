import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { I18nextProvider } from 'react-i18next'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../../i18n'
import type { PendingConfirmationDto } from '../types'
import { AutomationConfirmModal } from './AutomationConfirmModal'

// IPC invoke mock — intercepts and verifies the confirm_automation_command call
const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

function confirmationFixture(): PendingConfirmationDto {
  return {
    command_id: 'cmd-a11y-1',
    nonce: 'nonce-a11y-1',
    process_name: 'rm',
    args: ['-rf', '/tmp/test'],
    audit_level: 'Critical',
    requested_at: '2026-06-02T00:00:00Z',
  }
}

function renderModal(onDismiss = vi.fn()) {
  return render(
    <I18nextProvider i18n={i18n}>
      <AutomationConfirmModal confirmation={confirmationFixture()} onDismiss={onDismiss} />
    </I18nextProvider>,
  )
}

describe('AutomationConfirmModal a11y', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockInvoke.mockResolvedValue(undefined)
  })

  it('exposes a labelled, described modal dialog', () => {
    renderModal()
    const dialog = screen.getByRole('dialog')
    expect(dialog).toHaveAttribute('aria-modal', 'true')
    expect(dialog).toHaveAttribute('aria-labelledby')
    expect(dialog).toHaveAttribute('aria-describedby')

    const titleId = dialog.getAttribute('aria-labelledby')
    const descId = dialog.getAttribute('aria-describedby')
    expect(document.getElementById(titleId ?? '')).toHaveTextContent('Automation Confirmation')
    expect(document.getElementById(descId ?? '')).toHaveTextContent('rm')
  })

  it('autofocuses the Deny button as the safest default action', async () => {
    renderModal()
    const denyButton = screen.getByRole('button', { name: 'Deny' })
    await waitFor(() => {
      expect(document.activeElement).toBe(denyButton)
    })
  })

  it('treats Escape as an explicit deny (approved=false)', async () => {
    const onDismiss = vi.fn()
    renderModal(onDismiss)

    await act(async () => {
      fireEvent.keyDown(document, { key: 'Escape' })
    })

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('confirm_automation_command', {
        commandId: 'cmd-a11y-1',
        nonce: 'nonce-a11y-1',
        approved: false,
      })
    })
    await waitFor(() => {
      expect(onDismiss).toHaveBeenCalled()
    })
  })

  it('strips bidi/zero-width characters from the displayed process name (#6829)', () => {
    const rlo = String.fromCharCode(0x202e) // right-to-left override
    const zwsp = String.fromCharCode(0x200b) // zero-width space
    render(
      <I18nextProvider i18n={i18n}>
        <AutomationConfirmModal
          confirmation={{ ...confirmationFixture(), process_name: `evil${rlo}${zwsp}name` }}
          onDismiss={vi.fn()}
        />
      </I18nextProvider>,
    )
    const dialog = screen.getByRole('dialog')
    expect(dialog).toHaveTextContent('evilname')
    const text = dialog.textContent ?? ''
    expect(text.includes(rlo)).toBe(false)
    expect(text.includes(zwsp)).toBe(false)
  })
})
