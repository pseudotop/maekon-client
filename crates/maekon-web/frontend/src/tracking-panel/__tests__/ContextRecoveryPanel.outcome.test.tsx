/**
 * #9813 — recovery outcome copy must match what the reason actually says.
 *
 * The defect this pins: a missing assistant was rendered as "the suggestion
 * provider is unavailable right now", a sentence asserting that a provider
 * exists and that its failure is temporary. The reason says neither. For a
 * user who never set one up, the only instruction it carried was "wait", for a
 * recovery that never arrives — which is what the user reported.
 *
 * #12521 — the request now opens its own session, so the reason is
 * `chat_provider_not_ready` and the only useful step is provider setup.
 *
 * #12712 — except when only a provider CLI is ready: recovery cannot open one
 * with tools off (#12094), so `chat_cli_needs_open_chat` sends the user to Chat,
 * whose open session recovery reuses.
 *
 * Assertions go through `data-testid`, not copy: this panel ships in five
 * locales and a selector written against English text stops matching the moment
 * a translator touches it.
 */

import { act, fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { ToastContainer } from '../../components/ui/Toast'
import { clearToasts } from '../../hooks/useToast'
import i18n from '../../i18n'
import en from '../../i18n/locales/en.json'
import ko from '../../i18n/locales/ko.json'
import { ContextRecoveryPanel } from '../ContextRecoveryPanel'

const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

const RECOVERY_CMD = 'request_current_context_suggestions'

/** An `analysis_unavailable` turn carrying `reason`. */
function unavailableWith(reason: string) {
  return {
    status: 'analysis_unavailable',
    reason,
    admitted_count: 0,
    queue_count: 0,
    admitted_suggestion_ids: [],
    missing_permissions: [],
    provenance: null,
  }
}

function renderPanel() {
  return renderWithProviders(<ContextRecoveryPanel onBack={() => {}} />)
}

describe('ContextRecoveryPanel outcome copy (#9813)', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    mockInvoke.mockReset()
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  afterEach(() => {
    act(() => clearToasts())
  })

  it('does not call a missing provider a provider outage', async () => {
    // The whole slice in one assertion: this reason must NOT reach the
    // "offline, wait it out" copy.
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) return unavailableWith('chat_provider_not_ready')
      return []
    })

    renderPanel()

    expect(await screen.findByTestId('recovery-outcome-no-provider')).toBeInTheDocument()
    expect(screen.queryByTestId('recovery-outcome-provider-offline')).not.toBeInTheDocument()
  })

  it('tells the user what to do, rather than to wait', async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) return unavailableWith('chat_provider_not_ready')
      return []
    })

    renderPanel()

    const block = await screen.findByTestId('recovery-outcome-no-provider')
    // en.json: recovery.outcome.noProviderDetail — the copy names the step that
    // fixes it. A message with no action in it is the defect.
    expect(block.textContent).toMatch(/settings/i)
    // Opening Chat no longer helps: the request opens its own session (#12521).
    expect(block.textContent).not.toMatch(/open chat/i)
    // And it must not carry the claim that was wrong: a temporary provider fault.
    expect(block.textContent).not.toMatch(/unavailable right now/i)
  })

  it('still reports a real provider failure as an outage', async () => {
    // The fix must not swing the other way: these three DO describe a provider
    // that exists and failed, and "try later" is the honest reading.
    for (const reason of ['provider_unavailable', 'provider_error', 'provider_timeout']) {
      mockInvoke.mockReset()
      mockInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === RECOVERY_CMD) return unavailableWith(reason)
        return []
      })

      const { unmount } = renderPanel()
      expect(await screen.findByTestId('recovery-outcome-provider-offline'), reason).toBeInTheDocument()
      expect(screen.queryByTestId('recovery-outcome-no-provider')).not.toBeInTheDocument()
      unmount()
    }
  })

  it('leaves the insufficient-context outcome alone', async () => {
    // An unrecognised reason still means "nothing useful was on screen", which
    // is neither an outage nor a missing session.
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) return unavailableWith('empty_context')
      return []
    })

    renderPanel()

    expect(await screen.findByTestId('recovery-outcome-no-context')).toBeInTheDocument()
    expect(screen.queryByTestId('recovery-outcome-no-provider')).not.toBeInTheDocument()
    expect(screen.queryByTestId('recovery-outcome-provider-offline')).not.toBeInTheDocument()
  })

  it('keeps consent ahead of everything else', async () => {
    // Gate 1 of the two-gate chain the user hit. If a consent-blocked turn ever
    // rendered as a missing session, the panel would send them to Chat when the
    // actual block is a permission they have not granted.
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) {
        return {
          ...unavailableWith('screen_capture_consent_required'),
          status: 'consent_required',
          missing_permissions: ['screen_capture'],
        }
      }
      return []
    })

    renderPanel()

    expect(await screen.findByTestId('recovery-outcome-consent')).toBeInTheDocument()
    expect(screen.queryByTestId('recovery-outcome-no-provider')).not.toBeInTheDocument()
  })

  it('asks for the external-text permission when an HTTP path waits only for it', async () => {
    // #12521: an HTTP provider sends the prompt off-device, so without that
    // consent the request opens nothing and names the missing permission.
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) {
        return {
          ...unavailableWith('external_text_consent_required'),
          status: 'consent_required',
          missing_permissions: ['full_text_extraction'],
        }
      }
      return []
    })

    renderPanel()

    const block = await screen.findByTestId('recovery-outcome-consent')
    expect(block.textContent).toContain('full_text_extraction')
    expect(screen.queryByTestId('recovery-outcome-no-provider')).not.toBeInTheDocument()
  })

  it('sends a CLI-only user to Chat instead of calling the provider offline (#12712)', async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) return unavailableWith('chat_cli_needs_open_chat')
      return []
    })

    renderPanel()

    const block = await screen.findByTestId('recovery-outcome-open-chat')
    // en.json: recovery.outcome.openChatDetail names the steps that work.
    expect(block.textContent).toMatch(/open a chat/i)
    expect(block.textContent).toMatch(/settings/i)
    expect(block.textContent).not.toMatch(/unavailable right now/i)
    expect(screen.queryByTestId('recovery-outcome-provider-offline')).not.toBeInTheDocument()
    expect(screen.queryByTestId('recovery-outcome-no-provider')).not.toBeInTheDocument()
  })

  it('asks the backend once per mount', async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) return unavailableWith('chat_provider_not_ready')
      return []
    })

    renderPanel()
    await screen.findByTestId('recovery-outcome-no-provider')
    await waitFor(() => expect(mockInvoke.mock.calls.filter((c) => c[0] === RECOVERY_CMD)).toHaveLength(1))
  })

  const candidate = {
    id: 'candidate-1',
    state: 'PROPOSED',
    title: 'fixture next step',
    body: null,
    proposed_due: null,
    expires_at: '2026-12-01T00:00:00Z',
    source_kind: 'LOCAL_CURRENT_SCENE',
    source_lifecycle: 'ACTIVE',
    revision: 1,
  }
  const todo = {
    id: 'todo-1',
    state: 'CONFIRMED',
    title: 'fixture todo',
    body: null,
    due: null,
    revision: 1,
    created_at: '2026-11-16T00:00:00Z',
    updated_at: '2026-11-16T00:00:00Z',
  }
  const generated = { ...unavailableWith(''), status: 'generated', reason: null, admitted_count: 1, queue_count: 1 }

  it('keeps an error localized during a language change and retries only on user action', async () => {
    let reject = true
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) {
        if (reject) throw new Error('fixture diagnostic')
        return { ...unavailableWith(''), status: 'no_candidate' }
      }
      return []
    })
    renderPanel()
    const block = await screen.findByTestId('recovery-outcome-error')
    expect(block.querySelector('p')).toHaveTextContent(en.recovery.outcome.errorDetail)
    expect(screen.getByTestId('support-details')).not.toHaveAttribute('open')
    const calls = mockInvoke.mock.calls.length
    await act(async () => {
      await i18n.changeLanguage('ko')
    })
    expect(block.querySelector('p')).toHaveTextContent(ko.recovery.outcome.errorDetail)
    expect(mockInvoke).toHaveBeenCalledTimes(calls)
    reject = false
    fireEvent.click(screen.getByTestId('recovery-retry'))
    expect(await screen.findByTestId('recovery-outcome-no-candidate')).toBeInTheDocument()
    expect(screen.queryByTestId('recovery-outcome-error')).toBeNull()
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === RECOVERY_CMD)).toHaveLength(2)
  })

  it('does not describe a task-store failure as insufficient context or an empty to-do list', async () => {
    let reject = true
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) return generated
      if (cmd === 'list_task_candidates') {
        if (reject) throw new Error('fixture diagnostic')
        return [candidate]
      }
      return []
    })
    const { container } = renderPanel()
    await screen.findByTestId('recovery-outcome-task-error')
    expect(screen.queryByTestId('recovery-outcome-no-context')).toBeNull()
    expect(container.querySelector('[aria-labelledby="recovery-todos-heading"]')).toBeNull()
    reject = false
    fireEvent.click(screen.getByTestId('recovery-outcome-task-error-retry'))
    expect(await screen.findByTestId('recovery-card')).toBeInTheDocument()
    expect(screen.queryByTestId('recovery-outcome-task-error')).toBeNull()
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === RECOVERY_CMD)).toHaveLength(1)
  })

  it('shows task loading while the generated candidate is still being read', async () => {
    let complete!: (value: unknown[]) => void
    const pending = new Promise<unknown[]>((resolve) => {
      complete = resolve
    })
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) return generated
      if (cmd === 'list_task_candidates') return pending
      return []
    })
    renderPanel()
    expect(await screen.findByTestId('recovery-tasks-loading')).toHaveAttribute('aria-busy', 'true')
    expect(screen.queryByTestId('recovery-outcome-no-context')).toBeNull()
    await act(async () => {
      complete([candidate])
      await pending
    })
    expect(await screen.findByTestId('recovery-card')).toBeInTheDocument()
    expect(screen.queryByTestId('recovery-tasks-loading')).toBeNull()
  })

  it.each([
    ['confirm', 'confirm_task_candidate'],
    ['dismiss', 'dismiss_task_candidate'],
    ['transition', 'transition_todo'],
  ] as const)('reports a rejected %s without publishing a success notice', async (action, command) => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) return generated
      if (cmd === 'list_task_candidates') return [candidate]
      if (cmd === 'list_todos') return [todo]
      if (cmd === command) throw { code: 'future.task_failure', message: 'fixture diagnostic' }
      throw new Error(`unexpected test command: ${cmd}`)
    })
    renderWithProviders(
      <>
        <ContextRecoveryPanel onBack={() => {}} />
        <ToastContainer />
      </>,
    )
    await screen.findByTestId('recovery-card')
    const button =
      action === 'transition'
        ? within(screen.getByTestId('recovery-todo-todo-1')).getAllByRole('button')[0]
        : screen.getByTestId(`recovery-${action}`)
    fireEvent.click(button)
    await waitFor(() => expect(document.querySelector('[aria-live="polite"] p')).toHaveTextContent(en.errors.generic))
    expect(screen.queryByTestId('recovery-notice')).toBeNull()
    expect(screen.getByTestId('support-details-text')).toHaveTextContent('future.task_failure: fixture diagnostic')
    expect(screen.getByTestId('recovery-card')).toBeInTheDocument()
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === command)).toHaveLength(1)
  })

  it.each([
    [
      'confirm',
      'confirm_task_candidate',
      { outcome: 'already_transitioned', current_state: 'DISMISSED' },
      'actionConflict',
    ],
    ['dismiss', 'dismiss_task_candidate', { outcome: 'revision_conflict' }, 'actionConflict'],
    [
      'dismiss',
      'dismiss_task_candidate',
      { outcome: 'already_transitioned', current_state: 'CONFIRMED' },
      'actionConflict',
    ],
    ['dismiss', 'dismiss_task_candidate', { outcome: 'consent_required' }, 'actionConsentRequired'],
    ['transition', 'transition_todo', { outcome: 'source_unavailable' }, 'actionSourceUnavailable'],
  ] as const)('does not treat a domain refusal as %s success (%j)', async (action, command, outcome, key) => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) return generated
      if (cmd === 'list_task_candidates') return [candidate]
      if (cmd === 'list_todos') return [todo]
      if (cmd === command) return outcome
      throw new Error(`unexpected test command: ${cmd}`)
    })
    renderWithProviders(
      <>
        <ContextRecoveryPanel onBack={() => {}} />
        <ToastContainer />
      </>,
    )
    await screen.findByTestId('recovery-card')
    const button =
      action === 'transition'
        ? within(screen.getByTestId('recovery-todo-todo-1')).getAllByRole('button')[0]
        : screen.getByTestId(`recovery-${action}`)
    fireEvent.click(button)
    await waitFor(() =>
      expect(document.querySelector('[aria-live="polite"] p')).toHaveTextContent(i18n.t(`durableTasks.${key}`)),
    )
    expect(screen.queryByTestId('recovery-notice')).toBeNull()
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === command)).toHaveLength(1)
  })

  it.each([
    ['confirm', 'confirm_task_candidate', { outcome: 'confirmed' }, 'confirmedNotice'],
    [
      'confirm',
      'confirm_task_candidate',
      { outcome: 'already_transitioned', current_state: 'CONFIRMED' },
      'confirmedNotice',
    ],
    ['dismiss', 'dismiss_task_candidate', { outcome: 'dismissed' }, 'dismissedNotice'],
    [
      'dismiss',
      'dismiss_task_candidate',
      { outcome: 'already_transitioned', current_state: 'DISMISSED' },
      'dismissedNotice',
    ],
  ] as const)('keeps an applied or idempotent %s notice (%j)', async (action, command, outcome, key) => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === RECOVERY_CMD) return generated
      if (cmd === 'list_task_candidates') return [candidate]
      if (cmd === 'list_todos') return [todo]
      if (cmd === command) return outcome
      throw new Error(`unexpected test command: ${cmd}`)
    })
    renderWithProviders(
      <>
        <ContextRecoveryPanel onBack={() => {}} />
        <ToastContainer />
      </>,
    )
    await screen.findByTestId('recovery-card')
    fireEvent.click(screen.getByTestId(`recovery-${action}`))
    expect(await screen.findByTestId('recovery-notice')).toHaveTextContent(i18n.t(`recovery.${key}`))
    expect(document.querySelector('[aria-live="polite"] p')).toBeNull()
  })
})
