import { isIpcError } from '../../api/desktop'
import { type ErrorDisplay, supportDetailOf } from '../../i18n/errorDisplay'

/** `useErrorDisplay()`'s function: the localized sentence for any other failure. */
type DescribeError = (err: unknown, fallback?: string) => ErrorDisplay

interface ChatCreateErrorOptions {
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

/** Guidance that already names the cause and the fix needs no support detail. */
function guidance(text: string): ErrorDisplay {
  return { text, detail: null }
}

export function isMissingProviderCredentialError(error: unknown): boolean {
  if (isIpcError(error)) {
    // raw-error-text: matched against the backend marker, never rendered
    return error.code === 'auth.failed' && missingCredentialPattern.test(error.message)
  }

  const message = supportDetailOf(error) ?? ''
  return message.includes('auth.failed') && missingCredentialPattern.test(message)
}

/**
 * The create-session failure as the chat surface shows it (#13737): a known
 * cause becomes actionable guidance, anything else goes through the shared
 * display path so its English diagnostic stays collapsed as support detail.
 */
export function chatCreateErrorDisplay(
  error: unknown,
  describe: DescribeError,
  options: ChatCreateErrorOptions,
): ErrorDisplay {
  if (options.providerName && isMissingProviderCredentialError(error)) {
    return guidance(options.providerNotConfiguredMessage(options.providerName))
  }
  if (options.transport === 'local_llm' && isIpcError(error)) {
    if (error.code === 'not_found.resource_missing' && options.localModelMissingMessage) {
      return guidance(options.localModelMissingMessage)
    }
    if (error.code === 'service.unavailable' && options.localRuntimeUnavailableMessage) {
      return guidance(options.localRuntimeUnavailableMessage)
    }
    if (error.code === 'config.invalid' && options.localRuntimeInvalidMessage) {
      return guidance(options.localRuntimeInvalidMessage)
    }
  }
  return describe(error, options.fallback)
}

interface ChatSendErrorOptions {
  fullTextConsentRequiredMessage: string
  fallback: string
}

export function isFullTextConsentRequiredError(error: unknown): boolean {
  if (isIpcError(error)) {
    // raw-error-text: matched against the backend marker, never rendered
    return error.code === 'policy.denied' && fullTextConsentPattern.test(error.message)
  }

  const message = supportDetailOf(error) ?? ''
  return message.includes('policy.denied') && fullTextConsentPattern.test(message)
}

/** The send failure as the chat surface shows it; see `chatCreateErrorDisplay`. */
export function chatSendErrorDisplay(
  error: unknown,
  describe: DescribeError,
  options: ChatSendErrorOptions,
): ErrorDisplay {
  if (isFullTextConsentRequiredError(error)) {
    return guidance(options.fullTextConsentRequiredMessage)
  }
  return describe(error, options.fallback)
}
