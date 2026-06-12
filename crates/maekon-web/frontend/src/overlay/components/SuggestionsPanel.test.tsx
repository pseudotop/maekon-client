import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { SuggestionViewDto } from '../types'

// ADR-019 Follow-up #3 회귀 테스트:
// SuggestionsPanel 의 피드백 실패 토스트는 raw 영문 메시지가 아니라
// translateError 를 거친 현지화된 wire-error 메시지를 보여줘야 한다.

// showToast 를 캡처해 토스트 메시지를 단언한다.
const showToastMock = vi.fn()
vi.mock('./Toast', () => ({
  showToast: (message: string, type?: string) => showToastMock(message, type),
}))

// 리플레이 기록은 부수효과이므로 noop 처리.
vi.mock('../suggestionReplay', () => ({
  buildSuggestionReplayEvent: () => ({}),
  recordSuggestionReplayEvent: vi.fn().mockResolvedValue(undefined),
}))

// 현재 로케일을 제어하기 위한 가변 변수.
let currentLanguage = 'en'
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    // 기본값(2번째 인자)을 그대로 반환하는 단순 t.
    t: (_key: string, defaultValue?: string) => defaultValue ?? _key,
    i18n: {
      get language() {
        return currentLanguage
      },
    },
  }),
}))

// @tauri-apps/api/core 의 invoke 를 IpcError 로 reject 시킨다.
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
    is_read: false,
    reasoning: null,
  } as SuggestionViewDto
}

describe('SuggestionsPanel error localization (ADR-019 Follow-up #3)', () => {
  beforeEach(() => {
    showToastMock.mockClear()
    invokeMock.mockReset()
    currentLanguage = 'en'
    // refresh useEffect 가 onRefresh 를 호출하므로 무해한 resolve 로 둔다.
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  it('localizes an IpcError code into Korean when feedback submit fails (ko locale)', async () => {
    currentLanguage = 'ko'
    // submit_suggestion_feedback 호출만 reject 시킨다.
    invokeMock.mockRejectedValue({ code: 'config.invalid', message: 'bad value' })

    render(
      <SuggestionsPanel open suggestions={[makeSuggestion()]} onClose={() => {}} onRefresh={() => Promise.resolve()} />,
    )

    // Accept 버튼 클릭 → handleAction('accept') → invoke reject → 에러 토스트.
    const acceptBtn = await screen.findByText('suggestions.accept')
    fireEvent.click(acceptBtn)

    await waitFor(() => {
      expect(showToastMock).toHaveBeenCalled()
    })

    const [message, type] = showToastMock.mock.calls.at(-1) ?? []
    expect(type).toBe('error')
    // wire-errors.ko.json 의 config.invalid 템플릿이 적용되어야 한다 ('{message}' → 'bad value').
    expect(message).toContain('bad value')
    // raw 영문 fallback('Unknown error' 또는 코드 누락)이 아님을 보장.
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
    // en 템플릿 'Invalid configuration: {message}' 적용.
    expect(message).toContain('Invalid configuration')
    expect(message).toContain('bad value')
  })
})
