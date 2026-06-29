import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { I18nextProvider } from 'react-i18next'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../../i18n'
import type { CodexApprovalDto } from '../types'
import { CodexApprovalModal } from './CodexApprovalModal'

// IPC invoke mock — intercept respond_codex_approval and assert the decision.
const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

function approvalFixture(overrides: Partial<CodexApprovalDto> = {}): CodexApprovalDto {
  return {
    requestId: 4242,
    summary: 'command: git status',
    kind: 'command',
    processName: 'git',
    args: ['status'],
    diffLineCount: null,
    networkHost: null,
    ...overrides,
  }
}

function renderModal(onDismiss = vi.fn(), approval = approvalFixture()) {
  return render(
    <I18nextProvider i18n={i18n}>
      <CodexApprovalModal approval={approval} onDismiss={onDismiss} />
    </I18nextProvider>,
  )
}

describe('CodexApprovalModal (E21 #5044)', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockInvoke.mockResolvedValue(undefined)
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('renders command process + args and a labelled dialog', () => {
    renderModal()
    const dialog = screen.getByRole('dialog')
    expect(dialog).toHaveAttribute('aria-modal', 'true')
    const descId = dialog.getAttribute('aria-describedby')
    expect(document.getElementById(descId ?? '')).toHaveTextContent('git')
  })

  it('Accept invokes respond_codex_approval with decision accept', async () => {
    const onDismiss = vi.fn()
    renderModal(onDismiss)
    fireEvent.click(screen.getByRole('button', { name: 'Accept' }))
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('respond_codex_approval', {
        requestId: 4242,
        decision: 'accept',
      })
    })
    await waitFor(() => expect(onDismiss).toHaveBeenCalled())
  })

  it('Decline invokes respond_codex_approval with decision decline', async () => {
    renderModal()
    fireEvent.click(screen.getByRole('button', { name: 'Decline' }))
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('respond_codex_approval', {
        requestId: 4242,
        decision: 'decline',
      })
    })
  })

  it('Cancel maps to a fail-closed decision (cancel verb, never accept)', async () => {
    renderModal()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('respond_codex_approval', {
        requestId: 4242,
        decision: 'cancel',
      })
    })
    // Cancel must NEVER be the accept verb.
    expect(mockInvoke).not.toHaveBeenCalledWith('respond_codex_approval', {
      requestId: 4242,
      decision: 'accept',
    })
  })

  it('Escape resolves to decline (fail-closed)', async () => {
    renderModal()
    await act(async () => {
      fireEvent.keyDown(document, { key: 'Escape' })
    })
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('respond_codex_approval', {
        requestId: 4242,
        decision: 'decline',
      })
    })
  })

  it('auto-declines after the 30s timeout (fail-closed)', async () => {
    vi.useFakeTimers()
    render(
      <I18nextProvider i18n={i18n}>
        <CodexApprovalModal approval={approvalFixture()} onDismiss={vi.fn()} />
      </I18nextProvider>,
    )
    // advanceTimersByTimeAsync flushes the countdown ticks AND the async
    // dynamic-import in handleSubmit (real-timer waitFor would hang under fake
    // timers).
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000)
    })
    expect(mockInvoke).toHaveBeenCalledWith('respond_codex_approval', {
      requestId: 4242,
      decision: 'decline',
    })
  })

  it('renders a file_change diff line count', () => {
    renderModal(vi.fn(), approvalFixture({ kind: 'file_change', processName: null, args: [], diffLineCount: 7 }))
    expect(screen.getByText(/7 diff lines/)).toBeInTheDocument()
  })

  it('strips bidi/zero-width characters from displayed summary & processName (#6829)', () => {
    const rlo = String.fromCharCode(0x202e) // right-to-left override
    const zwsp = String.fromCharCode(0x200b) // zero-width space
    renderModal(vi.fn(), approvalFixture({ summary: `safe${rlo}cmd`, processName: `git${zwsp}hub` }))
    const dialog = screen.getByRole('dialog')
    expect(dialog).toHaveTextContent('safecmd')
    expect(dialog).toHaveTextContent('github')
    const text = dialog.textContent ?? ''
    expect(text.includes(rlo)).toBe(false)
    expect(text.includes(zwsp)).toBe(false)
  })
})
