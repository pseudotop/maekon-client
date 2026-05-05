import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { I18nextProvider } from 'react-i18next'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import { AutostartOnboardingPrompt } from './AutostartOnboardingPrompt'

// ---------------------------------------------------------------------------
// Mock: @tauri-apps/api/core + window.__TAURI_INTERNALS__
//
// AutostartOnboardingPrompt's invokeDesktop() uses dynamic import.
// Dual-mock per GeneralTab.test.tsx pattern: vi.mock intercepts module
// registry AND window.__TAURI_INTERNALS__ acts as fallback for the race.
// ---------------------------------------------------------------------------
const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

beforeEach(() => {
  mockInvoke.mockReset()
  ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: (...args: unknown[]) => mockInvoke(...args),
  }
})

const baseConfig = {
  prompt_state: { kind: 'pending' as const },
  productive_session_count: 1,
  last_session_id: null,
}

function renderPrompt(props: { onClose?: () => void; productiveSessionCount?: number } = {}) {
  const onClose = props.onClose ?? vi.fn()
  const config = {
    ...baseConfig,
    productive_session_count: props.productiveSessionCount ?? baseConfig.productive_session_count,
  }
  const result = render(
    <I18nextProvider i18n={i18n}>
      <AutostartOnboardingPrompt config={config} onClose={onClose} />
    </I18nextProvider>,
  )
  return { onClose, config, ...result }
}

describe('AutostartOnboardingPrompt', () => {
  it('renders title and body', () => {
    renderPrompt()
    // en.json: onboarding.autostart.title = "Start Maekon automatically?"
    expect(screen.getByText('Start Maekon automatically?')).toBeInTheDocument()
    // en.json: onboarding.autostart.body
    expect(screen.getByText(/Maekon works best when running in the background/i)).toBeInTheDocument()
  })

  it('Enable button invokes enable_autostart then mark_autostart_prompt_state dismissed', async () => {
    mockInvoke.mockResolvedValue(undefined)
    const onClose = vi.fn()
    renderPrompt({ onClose })

    // en.json: onboarding.autostart.enable_button = "Enable"
    fireEvent.click(screen.getByText('Enable'))

    await waitFor(() => {
      const calls = mockInvoke.mock.calls.map((c) => c[0])
      expect(calls).toContain('enable_autostart')
      expect(calls).toContain('mark_autostart_prompt_state')
    })

    // The mark call after Enable should set dismissed state
    const markCall = mockInvoke.mock.calls.find((c) => c[0] === 'mark_autostart_prompt_state')
    expect(markCall?.[1]).toEqual({ newState: { kind: 'dismissed' } })
    expect(onClose).toHaveBeenCalled()
  })

  it('Not now button sets snoozed with remind_after_session_count = count + 5', async () => {
    mockInvoke.mockResolvedValue(undefined)
    const onClose = vi.fn()
    renderPrompt({ onClose, productiveSessionCount: 3 })

    // en.json: onboarding.autostart.not_now_button = "Not now"
    fireEvent.click(screen.getByText('Not now'))

    await waitFor(() => {
      const markCall = mockInvoke.mock.calls.find((c) => c[0] === 'mark_autostart_prompt_state')
      expect(markCall).toBeDefined()
      expect(markCall?.[1]).toEqual({
        newState: { kind: 'snoozed', remind_after_session_count: 8 },
      })
      expect(onClose).toHaveBeenCalled()
    })
  })

  it("Don't ask again button sets dismissed state", async () => {
    mockInvoke.mockResolvedValue(undefined)
    const onClose = vi.fn()
    renderPrompt({ onClose })

    // en.json: onboarding.autostart.dismiss_button = "Don't ask again"
    fireEvent.click(screen.getByText("Don't ask again"))

    await waitFor(() => {
      const markCall = mockInvoke.mock.calls.find((c) => c[0] === 'mark_autostart_prompt_state')
      expect(markCall).toBeDefined()
      expect(markCall?.[1]).toEqual({ newState: { kind: 'dismissed' } })
      expect(onClose).toHaveBeenCalled()
    })
  })

  it('Escape key is treated as Not now (snoozed)', async () => {
    mockInvoke.mockResolvedValue(undefined)
    const onClose = vi.fn()
    // Dialog attaches keydown listener to document, not window
    renderPrompt({ onClose, productiveSessionCount: 2 })

    fireEvent.keyDown(document, { key: 'Escape' })

    await waitFor(() => {
      const markCall = mockInvoke.mock.calls.find((c) => c[0] === 'mark_autostart_prompt_state')
      expect(markCall).toBeDefined()
      expect(markCall?.[1].newState.kind).toBe('snoozed')
      expect(markCall?.[1].newState.remind_after_session_count).toBe(7)
      expect(onClose).toHaveBeenCalled()
    })
  })
})
