import { fireEvent, render, screen } from '@testing-library/react'
import { createInstance } from 'i18next'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { AiCapabilityReadiness, FeatureCapabilitySnapshot } from '../api/contracts'
import en from '../i18n/locales/en.json'
import es from '../i18n/locales/es.json'
import ja from '../i18n/locales/ja.json'
import ko from '../i18n/locales/ko.json'
import zhCN from '../i18n/locales/zh-CN.json'
import { AiReadinessNotice } from './AiReadinessNotice'

const translation = vi.hoisted(() => ({
  current: undefined as ((key: string, fallback?: string) => string) | undefined,
  language: undefined as string | undefined,
}))
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, fallback?: string) => translation.current?.(key, fallback) ?? fallback ?? key,
    i18n: { language: translation.language },
  }),
}))
afterEach(() => {
  translation.current = undefined
  translation.language = undefined
})

const LOCALES = { en, ko, ja, es, 'zh-CN': zhCN } as const

/** Binds the mocked `t` to the real catalog of one locale. */
async function useLocale(language: keyof typeof LOCALES) {
  const i18n = createInstance()
  await i18n.init({
    lng: language,
    resources: Object.fromEntries(Object.entries(LOCALES).map(([lng, copy]) => [lng, { translation: copy }])),
  })
  translation.current = (key, fallback) => i18n.t(key, { defaultValue: fallback ?? key })
  translation.language = language
  return LOCALES[language]
}

function readiness(overrides: Partial<AiCapabilityReadiness> = {}): AiCapabilityReadiness {
  return {
    capability_id: 'chat.subprocess',
    status: 'blocked',
    reason_code: 'provider_invocation_unverified',
    action: 'verify_provider_invocation',
    action_copy_key: 'aiReadiness.action.verifyProviderInvocation',
    dimensions: {
      compiled_capability: true,
      selected_access_mode: 'provider_subscription_cli',
      access_mode_compatible: true,
      endpoint_or_profile_configured: true,
      provider_detection: 'detected',
      provider_auth: 'ready',
      provider_invocation: 'unverified',
      model_availability: 'not_required',
      runtime_flag_enabled: true,
      consent: [],
      apply_requirement: 'restart',
      apply_pending: false,
      privacy_gate: 'enforced_at_invocation',
      egress_gate: 'enforced_at_invocation',
      budget_gate: 'enforced_at_invocation',
      audit_gate: 'enforced_at_invocation',
    },
    ...overrides,
  }
}

function snapshot(...items: AiCapabilityReadiness[]): FeatureCapabilitySnapshot {
  return {
    features: [],
    ai_readiness: items.length > 0 ? { contract_version: 1, capabilities: items } : undefined,
  }
}

/** The notice's text as the user reads it: everything except the collapsed support details. */
function visibleText(notice: HTMLElement): string {
  const copy = notice.cloneNode(true) as HTMLElement
  for (const details of copy.querySelectorAll('[data-testid="support-details"]')) details.remove()
  return copy.textContent ?? ''
}

describe('AiReadinessNotice shared consumer (#11735)', () => {
  it.each([
    'en',
    'ko',
    'ja',
    'es',
    'zh-CN',
  ] as const)('guides legacy HTTP settings back to provider selection in %s', async (language) => {
    const copy = await useLocale(language)
    const reason = 'http_chat_settings_resave_required'
    render(
      <AiReadinessNotice
        snapshot={snapshot(
          readiness({
            capability_id: 'chat.http_api',
            reason_code: reason,
            action: 'open_ai_settings',
            action_copy_key: 'aiReadiness.action.openAiSettings',
            dimensions: { ...readiness().dimensions, http_chat_settings_resave_required: true },
          }),
        )}
        capabilityIds={['chat.http_api']}
      />,
    )
    expect(screen.getByTestId('ai-readiness-notice')).toHaveTextContent(copy.aiReadiness.reason[reason])
    const link = screen.getByRole('link')
    expect(link).toHaveTextContent(copy.aiReadiness.action.openAiSettings)
    expect(link).toHaveAttribute('href', '/settings/ai-automation')
  })

  it('renders the exact backend reason and localized action route', () => {
    const { container } = render(
      <AiReadinessNotice snapshot={snapshot(readiness())} capabilityIds={['chat.subprocess']} />,
    )

    const item = container.querySelector('li[data-reason-code="provider_invocation_unverified"]')
    expect(item).toHaveAttribute('data-reason-code', 'provider_invocation_unverified')
    expect(screen.getByRole('link')).toHaveAttribute('href', '/settings/ai-automation')
    expect(screen.getByRole('link')).toHaveTextContent('aiReadiness.action.verifyProviderInvocation')
  })

  it('delegates route actions for consumers hosted outside the main router', () => {
    const onAction = vi.fn()
    render(
      <AiReadinessNotice snapshot={snapshot(readiness())} capabilityIds={['chat.subprocess']} onAction={onAction} />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'aiReadiness.action.verifyProviderInvocation' }))

    expect(onAction).toHaveBeenCalledWith('/settings/ai-automation')
  })

  it('hides ready capabilities by default and shows them in Settings overview mode', () => {
    const ready = readiness({
      status: 'ready',
      reason_code: 'ready',
      action: 'none',
      action_copy_key: 'aiReadiness.action.none',
      dimensions: { ...readiness().dimensions, provider_invocation: 'ready' },
    })
    const { rerender } = render(<AiReadinessNotice snapshot={snapshot(ready)} capabilityIds={['chat.subprocess']} />)
    expect(screen.queryByTestId('ai-readiness-notice')).toBeNull()

    rerender(<AiReadinessNotice snapshot={snapshot(ready)} capabilityIds={['chat.subprocess']} showReady />)
    expect(screen.getByTestId('ai-readiness-notice')).toHaveTextContent('ready')
  })

  it('fails closed when a legacy snapshot has no readiness contract', () => {
    const { container } = render(<AiReadinessNotice snapshot={snapshot()} capabilityIds={['daily_narrative']} />)

    expect(container.querySelector('li[data-reason-code="compiled_capability_missing"]')).not.toBeNull()
    expect(screen.queryByRole('link')).toBeNull()
    expect(screen.queryByText('aiReadiness.action.none')).toBeNull()
  })

  it('renders a blocker while the authoritative snapshot is unavailable', () => {
    const { container } = render(<AiReadinessNotice snapshot={undefined} capabilityIds={['ocr.suggestion_analysis']} />)

    expect(container.querySelector('li[data-reason-code="compiled_capability_missing"]')).not.toBeNull()
  })
})

describe('AiReadinessNotice error copy (#13737)', () => {
  it('keeps the internal reason code out of the visible text and in the support details', async () => {
    const copy = await useLocale('ko')
    const { container } = render(
      <AiReadinessNotice snapshot={snapshot(readiness())} capabilityIds={['chat.subprocess']} />,
    )

    const notice = screen.getByTestId('ai-readiness-notice')
    expect(visibleText(notice)).toContain(copy.aiReadiness.reason.provider_invocation_unverified)
    expect(visibleText(notice)).not.toContain('provider_invocation_unverified')
    expect(visibleText(notice)).not.toContain('chat.subprocess')
    expect(container.querySelector('li')).toHaveAttribute('data-reason-code', 'provider_invocation_unverified')
    expect(screen.getByTestId('support-details-text')).toHaveTextContent(
      'chat.subprocess: provider_invocation_unverified',
    )
  })

  it('groups capabilities that share a cause under one next action', async () => {
    const copy = await useLocale('ko')
    const consent = readiness({
      capability_id: 'daily_narrative',
      reason_code: 'consent_required',
      action: 'open_privacy_consent',
      action_copy_key: 'aiReadiness.action.openPrivacyConsent',
    })
    const { container } = render(
      <AiReadinessNotice
        snapshot={snapshot(readiness(), readiness({ capability_id: 'chat.http_api' }), consent)}
        capabilityIds={['chat.subprocess', 'chat.http_api', 'daily_narrative']}
      />,
    )

    const items = container.querySelectorAll('li')
    expect([...items].map((item) => item.getAttribute('data-capability-ids'))).toEqual([
      'chat.subprocess chat.http_api',
      'daily_narrative',
    ])
    expect(items[0]).toHaveTextContent(
      `${copy.aiReadiness.capability.chatSubprocess} 및 ${copy.aiReadiness.capability.chatHttpApi}`,
    )
    expect(screen.getAllByRole('link').map((link) => link.getAttribute('href'))).toEqual([
      '/settings/ai-automation',
      '/privacy/data',
    ])
    // Support still sees every capability, not only the first of each group.
    expect(screen.getByTestId('support-details-text').textContent).toBe(
      'chat.subprocess: provider_invocation_unverified\nchat.http_api: provider_invocation_unverified\ndaily_narrative: consent_required',
    )
  })

  it('never merges capabilities whose next action differs', () => {
    const { container } = render(
      <AiReadinessNotice
        snapshot={snapshot(
          readiness(),
          readiness({
            capability_id: 'chat.http_api',
            action: 'open_ai_settings',
            action_copy_key: 'aiReadiness.action.openAiSettings',
          }),
        )}
        capabilityIds={['chat.subprocess', 'chat.http_api']}
      />,
    )

    expect(container.querySelectorAll('li')).toHaveLength(2)
  })

  it('shows a generic sentence for a reason code this build has no copy for', async () => {
    const copy = await useLocale('ko')
    const reason = 'future_reason' as AiCapabilityReadiness['reason_code']
    render(
      <AiReadinessNotice
        snapshot={snapshot(
          readiness({ reason_code: reason, action: 'none', action_copy_key: 'aiReadiness.action.none' }),
        )}
        capabilityIds={['chat.subprocess']}
      />,
    )

    const notice = screen.getByTestId('ai-readiness-notice')
    expect(visibleText(notice)).toContain(copy.aiReadiness.unknownReason)
    expect(visibleText(notice)).not.toContain(reason)
    expect(screen.getByTestId('support-details-text')).toHaveTextContent(`chat.subprocess: ${reason}`)
  })
})
