import { isIpcError } from '../../api/desktop'
import { errorMessage } from './utils'

interface ChatCreateErrorMessageOptions {
  providerName?: string
  providerNotConfiguredMessage: (providerName: string) => string
  fallback: string
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
