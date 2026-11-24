import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../../i18n'
import type { SuggestionViewDto } from '../types'

// ADR-019 Follow-up #3 regression test:
// SuggestionsPanel's feedback-failure toast must show the localized wire-error
// message routed through translateError, not the raw English message.

// Capture showToast to assert on the toast message.
const showToastMock = vi.fn()
vi.mock('./Toast', () => ({
  showToast: (...args: unknown[]) => showToastMock(...args),
}))

vi.mock('../../components/AiReadinessNotice', () => ({
  AiReadinessNotice: ({ onAction }: { onAction?: (route: string) => void }) => (
    <button type="button" onClick={() => onAction?.('/settings/ai-automation')}>
      readiness action
    </button>
  ),
}))

// Replay recording is a side effect, so make it a noop.
vi.mock('../suggestionReplay', () => ({
  buildSuggestionReplayEvent: () => ({}),
  recordSuggestionReplayEvent: vi.fn().mockResolvedValue(undefined),
}))

const navigateMainWindowMock = vi.fn()
vi.mock('../mainWindowNavigation', () => ({
  navigateMainWindow: (...args: unknown[]) => navigateMainWindowMock(...args),
}))

// Mutable variable to control the current locale.
let currentLanguage = 'en'
vi.mock('react-i18next', async (original) => ({
  ...(await original<typeof import('react-i18next')>()),
  useTranslation: () => ({
    // Simple t that returns the default value (2nd argument) as-is.
    t: (_key: string, defaultValue?: string) =>
      _key === 'errors.generic' ? i18n.getFixedT(currentLanguage)(_key) : (defaultValue ?? _key),
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
    invokeMock.mockReset().mockResolvedValue(undefined)
    navigateMainWindowMock.mockReset().mockResolvedValue(undefined)
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
    expect(message).not.toContain('bad value')
    expect(showToastMock.mock.calls.at(-1)?.[3]).toBe('config.invalid: bad value')
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
    expect(message).not.toContain('bad value')
    expect(showToastMock.mock.calls.at(-1)?.[3]).toBe('config.invalid: bad value')
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

describe('SuggestionsPanel readiness navigation (#11735)', () => {
  it('restores the main window and emits the selected in-app route', async () => {
    const onClose = vi.fn()
    render(
      <SuggestionsPanel open suggestions={[makeSuggestion()]} onClose={onClose} onRefresh={() => Promise.resolve()} />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'readiness action' }))

    await waitFor(() => expect(navigateMainWindowMock).toHaveBeenCalledWith('/settings/ai-automation'))
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce())
  })
})

describe('SuggestionsPanel local analysis status (#11737)', () => {
  it('shows the latest metadata-only local generation status on the active review surface', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'get_suggestion_stats') {
        return Promise.resolve({
          latest_local_analysis: {
            status: 'generated',
            reason: 'generated',
            producer: 'app_switch',
            source: 'llm_local',
            observed_at: '2026-09-02T08:30:00Z',
            candidate_count: 1,
            queue_count: 1,
            missing_permissions: [],
          },
        })
      }
      return Promise.resolve(undefined)
    })

    render(
      <SuggestionsPanel open suggestions={[makeSuggestion()]} onClose={() => {}} onRefresh={() => Promise.resolve()} />,
    )

    expect(await screen.findByText('Local suggestions generated')).toBeInTheDocument()
    expect(screen.getByTestId('local-analysis-status')).toHaveAttribute('data-status', 'generated')
    expect(screen.getByTestId('local-analysis-status')).toHaveAttribute('data-candidate-count', '1')
    expect(screen.getByTestId('local-analysis-status')).toHaveAttribute('data-queue-count', '1')
  })
})

describe('Suggestion panel three error fanouts (#13756)', () => {
  it.each(
    ['feedback', 'explain', 'readiness'].flatMap((path) =>
      [
        ['en', 'Something went wrong. Try again.'],
        ['ko', '문제가 발생했습니다. 다시 시도하세요.'],
        ['ja', '問題が発生しました。もう一度お試しください。'],
        ['zh-CN', '出现了问题。请重试。'],
        ['es', 'Algo salió mal. Inténtelo de nuevo.'],
      ]
        .map(([language, generic]) => [path, language, 'future.panel', generic])
        .concat([
          [path, 'ja', 'policy.denied', 'ポリシーにより拒否されました'],
          [path, 'zh-CN', 'policy.denied', '被策略拒绝'],
          [path, 'es', 'policy.denied', 'Denegado por la política'],
        ]),
    ),
  )('separates the actual %s failure in %s (%s) and preserves recovery', async (path, language, code, generic) => {
    currentLanguage = language
    showToastMock.mockClear()
    invokeMock.mockReset().mockRejectedValue({ code, message: 'panel diagnostic' })
    navigateMainWindowMock.mockReset().mockRejectedValue({ code, message: 'panel diagnostic' })
    const onClose = vi.fn()
    const onRefresh = vi.fn().mockResolvedValue(undefined)
    render(<SuggestionsPanel open suggestions={[makeSuggestion()]} onClose={onClose} onRefresh={onRefresh} />)
    const button = await screen.findByText(
      path === 'feedback' ? 'suggestions.accept' : path === 'explain' ? 'suggestions.explain' : 'readiness action',
    )
    fireEvent.click(button)
    await waitFor(() => expect(showToastMock).toHaveBeenCalled())
    const [primary, type, duration, detail] = showToastMock.mock.calls.at(-1)!
    expect(primary).toContain(generic)
    expect(primary).not.toContain('panel diagnostic')
    expect([type, duration, detail]).toEqual(['error', undefined, `${code}: panel diagnostic`])
    if (path === 'readiness') {
      expect(navigateMainWindowMock).toHaveBeenCalledWith('/settings/ai-automation')
      expect(onClose).not.toHaveBeenCalled()
      navigateMainWindowMock.mockResolvedValue(undefined)
      fireEvent.click(button)
      await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1))
    } else {
      const command = path === 'feedback' ? 'submit_suggestion_feedback' : 'explain_suggestion_in_chat'
      expect(invokeMock.mock.calls.filter(([name]) => name === command)).toHaveLength(1)
      invokeMock.mockResolvedValue(undefined)
      fireEvent.click(button)
      await waitFor(() => expect(invokeMock.mock.calls.filter(([name]) => name === command)).toHaveLength(2))
    }
    currentLanguage = 'en'
  })
})
