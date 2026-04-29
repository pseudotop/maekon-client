/**
 * PII redaction for error reports.
 *
 * Strips file paths (privacy: reveals user's home directory) and common
 * secret patterns (API keys, JWTs, bearer tokens) from error messages and
 * stack traces before they are forwarded to Rust logging or shown in the
 * dev fallback UI.
 *
 * Defense-in-depth: even if a buggy component throws while rendering user
 * content, the most obvious credentials are masked before they hit the log
 * file or a bug-report bundle.
 */

const REDACTED = '[REDACTED]'

// File path patterns:
// - macOS / Linux: /Users/<name>/, /home/<name>/, /var/folders/<id>/
// - Windows: C:\Users\<name>\
//
// The Windows pattern's character class must exclude backslash, whitespace,
// and quote — written as `[^\\\s"]+`. The earlier `[^\\s"]+` was a typo that
// excluded the literal letter 's' instead of whitespace, leaving any
// username containing 's' (Sam, Steven, Tess, ...) un-redacted.
const HOME_PATH_PATTERNS: ReadonlyArray<RegExp> = [
  /\/Users\/[^/\s"]+/g,
  /\/home\/[^/\s"]+/g,
  /\/var\/folders\/[a-zA-Z0-9_+/]+/g,
  /[A-Z]:\\Users\\[^\\\s"]+/g,
]

// Common secret patterns:
// - sk-... (OpenAI / Anthropic / generic), 20+ alphanumeric after the prefix
// - Bearer tokens
// - JWT shape (eyJ + base64url)
// - Generic "key=" / "token=" pairs
const SECRET_PATTERNS: ReadonlyArray<RegExp> = [
  /\bsk-[A-Za-z0-9_-]{16,}/g,
  /\bBearer\s+[A-Za-z0-9._~+/-]+={0,2}/gi,
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/g,
  /\b(?:api[_-]?key|password|token|secret)\s*[=:]\s*"?[^\s"]{6,}/gi,
]

/**
 * Mask file paths and obvious secrets in a string. Returns the original
 * string with sensitive substrings replaced with [REDACTED].
 */
export function redact(input: string | undefined | null): string | undefined {
  if (input == null) return undefined
  let output = input
  for (const pattern of HOME_PATH_PATTERNS) {
    output = output.replace(pattern, '~')
  }
  for (const pattern of SECRET_PATTERNS) {
    output = output.replace(pattern, REDACTED)
  }
  return output
}
