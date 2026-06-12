import { render } from '@testing-library/react'
import { I18nextProvider } from 'react-i18next'
import { describe, expect, it } from 'vitest'
import i18n from '../../../i18n'
import type { CoachingPayload, SuggestionGuiAnchorPayload } from '../../types'
import CoachingPopup from '../CoachingPopup'
import { SuggestionBadge } from '../SuggestionBadge'

// 스크린리더 안내(aria-live)가 코칭 팝업/제안 배지에 존재하는지 검증한다 (#4822).
// Toast 의 polite-region 패턴을 인라인 재사용했는지 확인.

function renderWithI18n(node: React.ReactElement) {
  return render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>)
}

const coaching: CoachingPayload = {
  message_id: 'm-1',
  profile: 'default',
  trigger_type: 'idle',
  text: 'Take a short break.',
  auto_dismiss_secs: 15,
  explanation: '',
}

const anchor: SuggestionGuiAnchorPayload = {
  active_app: 'Calculator',
  target_entity: { label: 'field', value: '42' } as SuggestionGuiAnchorPayload['target_entity'],
  highlight: { bounds: { x: 10, y: 20, width: 30, height: 40 } } as SuggestionGuiAnchorPayload['highlight'],
}

describe('overlay aria-live announcements (#4822)', () => {
  it('CoachingPopup announces its message via an aria-live polite region', () => {
    const { container } = renderWithI18n(<CoachingPopup message={coaching} autoDismissSecs={15} />)
    const live = container.querySelector('[aria-live="polite"]')
    expect(live).not.toBeNull()
    expect(live?.textContent).toContain('Take a short break.')
  })

  it('SuggestionBadge (plain) announces the count via an aria-live polite region', () => {
    const { container } = renderWithI18n(<SuggestionBadge count={3} onClick={() => {}} />)
    const live = container.querySelector('[aria-live="polite"]')
    expect(live).not.toBeNull()
    expect(live?.getAttribute('aria-atomic')).toBe('true')
  })

  it('SuggestionBadge (anchored) announces the count via an aria-live polite region', () => {
    const { container } = renderWithI18n(<SuggestionBadge count={2} onClick={() => {}} anchor={anchor} />)
    const live = container.querySelector('[aria-live="polite"]')
    expect(live).not.toBeNull()
    expect(live?.getAttribute('aria-atomic')).toBe('true')
  })
})
