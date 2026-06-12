import type { SuggestionViewDto } from './types'

const SENSITIVE_PATTERNS: Array<{
  pattern: RegExp
  replacement: string | ((match: string) => string)
}> = [
  {
    pattern: /\b(?:password|passcode|pwd|secret|token|api[_ -]?key)\s*[:=]\s*["']?[^"'\s,;.]+["']?/gi,
    replacement: (match: string) => {
      const [label] = match.split(/[:=]/)
      return `${label.trim()}: [REDACTED_SECRET]`
    },
  },
  {
    pattern: /\b\d(?:[ -]?\d){12,18}\b/g,
    replacement: '[REDACTED_CARD]',
  },
  {
    pattern: /\b\d{3}-\d{2}-\d{4}\b/g,
    replacement: '[REDACTED_ID]',
  },
  {
    pattern: /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/gi,
    replacement: '[REDACTED_EMAIL]',
  },
]

function replaceSensitivePattern(
  value: string,
  pattern: RegExp,
  replacement: string | ((match: string) => string),
): string {
  if (typeof replacement === 'function') {
    return value.replace(pattern, replacement)
  }
  return value.replace(pattern, replacement)
}

export function redactSensitiveText(value: string): string {
  return SENSITIVE_PATTERNS.reduce(
    (next, { pattern, replacement }) => replaceSensitivePattern(next, pattern, replacement),
    value,
  )
}

export function redactSuggestionView(item: SuggestionViewDto): SuggestionViewDto {
  return {
    ...item,
    title: redactSensitiveText(item.title),
    body: redactSensitiveText(item.body),
    reasoning: item.reasoning ? redactSensitiveText(item.reasoning) : item.reasoning,
  }
}

export function redactSuggestionViews(items: SuggestionViewDto[]): SuggestionViewDto[] {
  return items.map(redactSuggestionView)
}
