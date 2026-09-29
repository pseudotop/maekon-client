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

import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
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
  beforeEach(() => {
    mockInvoke.mockReset()
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
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
})
