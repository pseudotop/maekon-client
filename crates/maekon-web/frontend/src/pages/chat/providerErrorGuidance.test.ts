import { describe, expect, it } from 'vitest'
import { describeErrorForDisplay } from '../../i18n/errorDisplay'
import {
  chatCreateErrorDisplay,
  chatSendErrorDisplay,
  isFullTextConsentRequiredError,
  isMissingProviderCredentialError,
} from './providerErrorGuidance'

const backendMessage = "no credential available for surface 'provider_surface.anthropic.direct_api'"

/** The shared display path as a screen binds it, with keys standing in for copy. */
const displayError = (err: unknown, fallback?: string) => describeErrorForDisplay(err, (key) => key, 'en', fallback)

describe('chat provider error guidance', () => {
  it('recognizes typed and legacy missing-credential errors', () => {
    expect(isMissingProviderCredentialError({ code: 'auth.failed', message: backendMessage })).toBe(true)
    expect(isMissingProviderCredentialError(`Authentication error [auth.failed]: ${backendMessage}`)).toBe(true)
  })

  it('returns localized actionable provider guidance without the internal surface id', () => {
    const display = chatCreateErrorDisplay({ code: 'auth.failed', message: backendMessage }, displayError, {
      providerName: 'Anthropic API',
      providerNotConfiguredMessage: (provider) =>
        `${provider}이(가) 설정되지 않았습니다. 설정 → AI & 자동화에서 자격 증명을 추가한 뒤 다시 시도하세요.`,
      fallback: 'AI 세션을 만들지 못했습니다.',
    })

    expect(display.text).toContain('Anthropic API')
    expect(display.text).toContain('설정 → AI & 자동화')
    expect(display.text).not.toContain('provider_surface.')
    expect(display.detail).toBeNull()
  })

  it('shows the fallback sentence for unrelated errors and keeps their text as support detail (#13737)', () => {
    const display = chatCreateErrorDisplay(new Error('provider timed out'), displayError, {
      providerName: 'Anthropic API',
      providerNotConfiguredMessage: () => 'not used',
      fallback: 'Could not create a session.',
    })

    expect(display).toEqual({ text: 'Could not create a session.', detail: 'provider timed out' })
  })

  it.each([
    ['service.unavailable', 'Ollama를 시작하세요.'],
    ['config.invalid', 'Ollama endpoint를 선택하세요.'],
    ['not_found.resource_missing', 'Ollama 모델을 설치하세요.'],
  ])('maps Local LLM create error %s to localized pre-send guidance', (code, expected) => {
    const display = chatCreateErrorDisplay({ code, message: 'backend diagnostic' }, displayError, {
      providerName: 'Ollama',
      providerNotConfiguredMessage: () => 'not used',
      fallback: '세션을 만들지 못했습니다.',
      transport: 'local_llm',
      localRuntimeUnavailableMessage: 'Ollama를 시작하세요.',
      localRuntimeInvalidMessage: 'Ollama endpoint를 선택하세요.',
      localModelMissingMessage: 'Ollama 모델을 설치하세요.',
    })

    expect(display.text).toBe(expected)
  })

  it('maps full-text policy denial to actionable Privacy guidance', () => {
    const error = {
      code: 'policy.denied',
      message: 'External LLM blocked: full text extraction consent is required',
    }

    expect(isFullTextConsentRequiredError(error)).toBe(true)
    expect(
      chatSendErrorDisplay(error, displayError, {
        fullTextConsentRequiredMessage:
          '개인정보 → 데이터 관리에서 AI 텍스트 처리 및 외부 제공자를 켠 뒤 다시 시도하세요.',
        fallback: '메시지를 보내지 못했습니다.',
      }).text,
    ).toContain('개인정보 → 데이터 관리')
  })

  it('keeps a denial without the consent marker on the shared path', () => {
    const display = chatSendErrorDisplay({ code: 'policy.denied', message: 'egress budget exhausted' }, displayError, {
      fullTextConsentRequiredMessage: 'not used',
      fallback: '메시지를 보내지 못했습니다.',
    })

    // The catalog sentence for the code, without its English `{message}` slot.
    expect(display.text).toBe('Policy denied')
    expect(display.detail).toBe('policy.denied: egress budget exhausted')
  })
})
