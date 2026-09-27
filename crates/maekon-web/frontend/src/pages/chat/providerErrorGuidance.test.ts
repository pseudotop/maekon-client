import { describe, expect, it } from 'vitest'
import {
  chatCreateErrorMessage,
  chatSendErrorMessage,
  isFullTextConsentRequiredError,
  isMissingProviderCredentialError,
} from './providerErrorGuidance'

const backendMessage = "no credential available for surface 'provider_surface.anthropic.direct_api'"

describe('chat provider error guidance', () => {
  it('recognizes typed and legacy missing-credential errors', () => {
    expect(isMissingProviderCredentialError({ code: 'auth.failed', message: backendMessage })).toBe(true)
    expect(isMissingProviderCredentialError(`Authentication error [auth.failed]: ${backendMessage}`)).toBe(true)
  })

  it('returns localized actionable provider guidance without the internal surface id', () => {
    const message = chatCreateErrorMessage(
      { code: 'auth.failed', message: backendMessage },
      {
        providerName: 'Anthropic API',
        providerNotConfiguredMessage: (provider) =>
          `${provider}이(가) 설정되지 않았습니다. 설정 → AI & 자동화에서 자격 증명을 추가한 뒤 다시 시도하세요.`,
        fallback: 'AI 세션을 만들지 못했습니다.',
      },
    )

    expect(message).toContain('Anthropic API')
    expect(message).toContain('설정 → AI & 자동화')
    expect(message).not.toContain('provider_surface.')
  })

  it('preserves fallback handling for unrelated errors', () => {
    const message = chatCreateErrorMessage(new Error('provider timed out'), {
      providerName: 'Anthropic API',
      providerNotConfiguredMessage: () => 'not used',
      fallback: 'Could not create a session.',
    })

    expect(message).toBe('provider timed out')
  })

  it.each([
    ['service.unavailable', 'Ollama를 시작하세요.'],
    ['config.invalid', 'Ollama endpoint를 선택하세요.'],
    ['not_found.resource_missing', 'Ollama 모델을 설치하세요.'],
  ])('maps Local LLM create error %s to localized pre-send guidance', (code, expected) => {
    const message = chatCreateErrorMessage(
      { code, message: 'backend diagnostic' },
      {
        providerName: 'Ollama',
        providerNotConfiguredMessage: () => 'not used',
        fallback: '세션을 만들지 못했습니다.',
        transport: 'local_llm',
        localRuntimeUnavailableMessage: 'Ollama를 시작하세요.',
        localRuntimeInvalidMessage: 'Ollama endpoint를 선택하세요.',
        localModelMissingMessage: 'Ollama 모델을 설치하세요.',
      },
    )

    expect(message).toBe(expected)
  })

  it('maps full-text policy denial to actionable Privacy guidance', () => {
    const error = {
      code: 'policy.denied',
      message: 'External LLM blocked: full text extraction consent is required',
    }

    expect(isFullTextConsentRequiredError(error)).toBe(true)
    expect(
      chatSendErrorMessage(error, {
        fullTextConsentRequiredMessage: '개인정보 → 동의에서 AI 텍스트 처리 및 외부 제공자를 켠 뒤 다시 시도하세요.',
        fallback: '메시지를 보내지 못했습니다.',
      }),
    ).toContain('개인정보 → 동의')
  })
})
