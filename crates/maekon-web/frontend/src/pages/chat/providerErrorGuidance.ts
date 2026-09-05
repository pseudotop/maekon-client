import { isIpcError } from '../../api/desktop'
import { errorMessage } from './utils'

interface ChatCreateErrorMessageOptions {
  providerName?: string
  providerNotConfiguredMessage: (providerName: string) => string
  fallback: string
  transport?: 'subprocess' | 'http_api' | 'local_llm'
  localRuntimeUnavailableMessage?: string
  localRuntimeInvalidMessage?: string
  localModelMissingMessage?: string
}

const missingCredentialPattern = /no credential available for surface\b/i
const fullTextConsentPattern = /full text extraction consent/i

export function isMissingProviderCredentialError(error: unknown): boolean {
  if (isIpcError(error)) {
    return error.code === 'auth.failed' && missingCredentialPattern.test(error.message)
  }

  const message = errorMessage(error, '')
  return message.includes('auth.failed') && missingCredentialPattern.test(message)
}

export function chatCreateErrorMessage(error: unknown, options: ChatCreateErrorMessageOptions): string {
  if (options.providerName && isMissingProviderCredentialError(error)) {
    return options.providerNotConfiguredMessage(options.providerName)
  }
  if (options.transport === 'local_llm' && isIpcError(error)) {
    if (error.code === 'not_found.resource_missing' && options.localModelMissingMessage) {
      return options.localModelMissingMessage
    }
    if (error.code === 'service.unavailable' && options.localRuntimeUnavailableMessage) {
      return options.localRuntimeUnavailableMessage
    }
    if (error.code === 'config.invalid' && options.localRuntimeInvalidMessage) {
      return options.localRuntimeInvalidMessage
    }
  }
  return errorMessage(error, options.fallback)
}

interface ChatSendErrorMessageOptions {
  fullTextConsentRequiredMessage: string
  fallback: string
}

export function isFullTextConsentRequiredError(error: unknown): boolean {
  if (isIpcError(error)) {
    return error.code === 'policy.denied' && fullTextConsentPattern.test(error.message)
  }

  const message = errorMessage(error, '')
  return message.includes('policy.denied') && fullTextConsentPattern.test(message)
}

export function chatSendErrorMessage(error: unknown, options: ChatSendErrorMessageOptions): string {
  if (isFullTextConsentRequiredError(error)) {
    return options.fullTextConsentRequiredMessage
  }
  return errorMessage(error, options.fallback)
}
