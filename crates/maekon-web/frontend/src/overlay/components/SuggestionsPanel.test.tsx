import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { SuggestionViewDto } from '../types'

// ADR-019 Follow-up #3 regression test:
// SuggestionsPanel's feedback-failure toast must show the localized wire-error
// message routed through translateError, not the raw English message.

// Capture showToast to assert on the toast message.
const showToastMock = vi.fn()
vi.mock('./Toast', () => ({
  showToast: (message: string, type?: string) => showToastMock(message, type),
}))

// Replay recording is a side effect, so make it a noop.
vi.mock('../suggestionReplay', () => ({
  buildSuggestionReplayEvent: () => ({}),
  recordSuggestionReplayEvent: vi.fn().mockResolvedValue(undefined),
}))

// Mutable variable to control the current locale.
let currentLanguage = 'en'
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    // Simple t that returns the default value (2nd argument) as-is.
    t: (_key: string, defaultValue?: string) => defaultValue ?? _key,
    i18n: {
      get language() {
        return currentLanguage
      },
    },
  }),
}))

// Make @tauri-apps/api/core's invoke reject with an IpcError.
const invokeMock = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

import { SuggestionsPanel } from './SuggestionsPanel'

function makeSuggestion(): SuggestionViewDto {
  return {
    id: 'sug-1',
    title: 'Use keyboard shortcuts',
    body: 'You switch apps frequently.',
    priority: 'medium',
    category: 'productivity',
    source: 'server',
    confidence_score: 0.85,
    created_at: '2026-03-27T10:30:00Z',
    reasoning: null,
  } as SuggestionViewDto
}

describe('SuggestionsPanel error localization (ADR-019 Follow-up #3)', () => {
  beforeEach(() => {
    showToastMock.mockClear()
    invokeMock.mockReset()
    currentLanguage = 'en'
    // The refresh useEffect calls onRefresh, so keep it a harmless resolve.
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  it('localizes an IpcError code into Korean when feedback submit fails (ko locale)', async () => {
    currentLanguage = 'ko'
    // Reject only the submit_suggestion_feedback call.
    invokeMock.mockRejectedValue({ code: 'config.invalid', message: 'bad value' })

    render(
      <SuggestionsPanel open suggestions={[makeSuggestion()]} onClose={() => {}} onRefresh={() => Promise.resolve()} />,
    )

    // Click the Accept button -> handleAction('accept') -> invoke reject -> error toast.
    const acceptBtn = await screen.findByText('suggestions.accept')
    fireEvent.click(acceptBtn)

    await waitFor(() => {
      expect(showToastMock).toHaveBeenCalled()
    })

    const [message, type] = showToastMock.mock.calls.at(-1) ?? []
    expect(type).toBe('error')
    // The config.invalid template from wire-errors.ko.json should be applied ('{message}' -> 'bad value').
    expect(message).toContain('bad value')
    // Ensure it is not the raw English fallback ('Unknown error' or a missing code).
    expect(message).not.toBe('Feedback failed: bad value')
    expect(message).not.toContain('Unknown error')
  })

  it('falls back to English translation for ko-unknown but en-known code (en locale)', async () => {
    currentLanguage = 'en'
    invokeMock.mockRejectedValue({ code: 'config.invalid', message: 'bad value' })

    render(
      <SuggestionsPanel open suggestions={[makeSuggestion()]} onClose={() => {}} onRefresh={() => Promise.resolve()} />,
    )

    const acceptBtn = await screen.findByText('suggestions.accept')
    fireEvent.click(acceptBtn)

    await waitFor(() => {
      expect(showToastMock).toHaveBeenCalled()
    })

    const [message, type] = showToastMock.mock.calls.at(-1) ?? []
    expect(type).toBe('error')
    // The en template 'Invalid configuration: {message}' is applied.
    expect(message).toContain('Invalid configuration')
    expect(message).toContain('bad value')
  })
})

describe('SuggestionsPanel visual hierarchy (#8474)', () => {
  it('keeps replay states neutral instead of coloring every completed step as an accent', async () => {
    render(
      <SuggestionsPanel open suggestions={[makeSuggestion()]} onClose={() => {}} onRefresh={() => Promise.resolve()} />,
    )

    const phases = screen.getByTestId('suggestion-replay-trail').querySelectorAll('[data-rum-phase]')
    expect(phases).toHaveLength(4)
    for (const phase of phases) {
      expect(phase.className).not.toMatch(/brand/)
    }

    expect(screen.getByTestId('suggestion-replay-trail').querySelector('[data-rum-phase="target"]')).toHaveClass(
      'bg-transparent',
      'text-content-tertiary',
    )
    for (const phase of ['proposal', 'consent', 'audit']) {
      expect(screen.getByTestId('suggestion-replay-trail').querySelector(`[data-rum-phase="${phase}"]`)).toHaveClass(
        'bg-surface-muted',
        'text-content-secondary',
      )
    }

    await waitFor(() => expect(screen.queryByText('Refreshing suggestions...')).not.toBeInTheDocument())
  })
})
