export const ALLOWED_WEB_VERIFICATION_ROUTES = [
  { path: '/', expectedPath: '/overview', expectedContentSelector: '#section-overview' },
  { path: '/timeline', expectedPath: '/timeline/all', expectedContentSelector: '#section-all' },
  { path: '/reports', expectedPath: '/reports/activity', expectedContentSelector: '#section-activity' },
  { path: '/automation', expectedPath: '/automation/policies', expectedContentSelector: '#section-policies' },
  { path: '/settings', expectedPath: '/settings/general', expectedContentSelector: '#settings-form' },
] as const

export const ALLOWED_WEB_EVIDENCE_ARTIFACTS = [
  'redacted_dom_snapshot',
  'cropped_region_screenshot',
  'console_summary',
  'network_summary',
  'geometry_summary',
  'evidence_manifest',
] as const

export type WebVerificationRoutePath = (typeof ALLOWED_WEB_VERIFICATION_ROUTES)[number]['path']
export type WebEvidenceArtifactType = (typeof ALLOWED_WEB_EVIDENCE_ARTIFACTS)[number]
export type RenderedStateStatus = 'passed' | 'failed'
export type PrivacyPolicyViolation =
  | 'artifact.broad_screenshot.rejected'
  | 'artifact.unknown.rejected'
  | 'field.rawWindowTitle.omitted'
  | 'field.typedUserText.omitted'
  | 'field.rawLabel.omitted'
  | 'field.secret.redacted'

type JsonPrimitive = string | number | boolean | null
type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue }

export interface RenderedStateCheck {
  id: string
  passed: boolean
  diagnosticCode: string
  [key: string]: unknown
}

export interface WebEvidenceArtifactRequest {
  type: string
  [key: string]: unknown
}

export interface RenderedStateManifestInput {
  route: WebVerificationRoutePath
  finalUrl: string
  status: RenderedStateStatus
  checks: RenderedStateCheck[]
  artifacts: WebEvidenceArtifactRequest[]
}

export interface RenderedStateManifest {
  version: 'automation.web.rendered_state.v1'
  policy: 'automation.gui.permission_evidence.v1'
  route: WebVerificationRoutePath
  finalUrl: string
  status: RenderedStateStatus
  checks: JsonValue[]
  artifacts: JsonValue[]
  diagnosticBundle?: {
    codes: string[]
    artifactTypes: WebEvidenceArtifactType[]
  }
}

export function assertPrivacySafeArtifactRequest(type: string): asserts type is WebEvidenceArtifactType {
  if (type === 'broad_screenshot') {
    throw new Error('broad_screenshot is rejected by automation.gui.permission_evidence.v1')
  }
  if (!isAllowedArtifactType(type)) {
    throw new Error(`unknown web evidence artifact rejected: ${type}`)
  }
}

export function buildRenderedStateManifest(input: RenderedStateManifestInput): RenderedStateManifest {
  assertAllowedRoute(input.route)

  const artifacts = input.artifacts.map((artifact) => {
    assertPrivacySafeArtifactRequest(artifact.type)
    return sanitizeEvidenceRecord(artifact)
  })
  const checks = input.checks.map((check) => sanitizeEvidenceRecord(check))
  const diagnosticCodes = input.checks
    .filter((check) => !check.passed)
    .map((check) => check.diagnosticCode)
    .filter(Boolean)

  return {
    version: 'automation.web.rendered_state.v1',
    policy: 'automation.gui.permission_evidence.v1',
    route: input.route,
    finalUrl: sanitizeUrl(input.finalUrl),
    status: input.status,
    checks,
    artifacts,
    ...(input.status === 'failed'
      ? {
          diagnosticBundle: {
            codes: [...new Set(diagnosticCodes)],
            artifactTypes: artifacts
              .map((artifact) => {
                if (
                  isJsonObject(artifact) &&
                  typeof artifact.type === 'string' &&
                  isAllowedArtifactType(artifact.type)
                ) {
                  return artifact.type
                }
                return null
              })
              .filter((type): type is WebEvidenceArtifactType => type !== null),
          },
        }
      : {}),
  }
}

export function sanitizeEvidenceRecord(record: Record<string, unknown>): { [key: string]: JsonValue } {
  const sanitized: { [key: string]: JsonValue } = {}
  for (const [key, value] of Object.entries(record)) {
    sanitized[key] = sanitizeEvidenceValue(key, value)
  }
  return sanitized
}

export function redactEvidenceString(value: string): string {
  if (looksLikeUrl(value)) {
    return sanitizeUrl(value)
  }

  return value
    .replace(/Bearer\s+[A-Za-z0-9._~+/=-]+/gi, 'Bearer [redacted]')
    .replace(/\b(token|api[_-]?key|password|secret)=([^\s&#]+)/gi, '$1=[redacted]')
}

export function stableEvidenceToken(value: string): string {
  const normalized = value.trim()
  if (!isSafeStableIdentifier(normalized)) {
    return '[redacted:unstable-identifier]'
  }

  let hashA = 0x811c9dc5
  let hashB = 0x9e3779b9
  for (let index = 0; index < normalized.length; index += 1) {
    const code = normalized.charCodeAt(index)
    hashA ^= code
    hashA = Math.imul(hashA, 0x01000193)
    hashB ^= code + index
    hashB = Math.imul(hashB, 0x85ebca6b)
  }

  return `stable:${toHex32(hashA)}${toHex32(hashB)}`
}

export function findPrivacyPolicyViolations(payload: unknown): PrivacyPolicyViolation[] {
  const violations: PrivacyPolicyViolation[] = []

  function add(violation: PrivacyPolicyViolation) {
    if (!violations.includes(violation)) {
      violations.push(violation)
    }
  }

  function visit(value: unknown) {
    if (Array.isArray(value)) {
      for (const item of value) {
        visit(item)
      }
      return
    }

    if (!isPlainRecord(value)) {
      return
    }

    if (value.type === 'broad_screenshot') {
      add('artifact.broad_screenshot.rejected')
    } else if (
      typeof value.type === 'string' &&
      value.type.includes('screenshot') &&
      !isAllowedArtifactType(value.type)
    ) {
      add('artifact.unknown.rejected')
    }

    for (const [key, child] of Object.entries(value)) {
      const fieldViolation = violationForField(key, child)
      if (fieldViolation) {
        add(fieldViolation)
      }
      visit(child)
    }
  }

  visit(payload)
  return violations
}

function assertAllowedRoute(route: string): asserts route is WebVerificationRoutePath {
  if (!ALLOWED_WEB_VERIFICATION_ROUTES.some((allowedRoute) => allowedRoute.path === route)) {
    throw new Error(`web verification route is not allowed: ${route}`)
  }
}

function isAllowedArtifactType(type: string): type is WebEvidenceArtifactType {
  return ALLOWED_WEB_EVIDENCE_ARTIFACTS.includes(type as WebEvidenceArtifactType)
}

function sanitizeEvidenceValue(key: string, value: unknown): JsonValue {
  const replacement = replacementForField(key)
  if (replacement) {
    return replacement
  }

  if (typeof value === 'string') {
    return key.toLowerCase().includes('url') ? sanitizeUrl(value) : redactEvidenceString(value)
  }

  if (typeof value === 'number' || typeof value === 'boolean' || value === null) {
    return value
  }

  if (Array.isArray(value)) {
    return value.map((item) => sanitizeEvidenceValue('', item))
  }

  if (isPlainRecord(value)) {
    return sanitizeEvidenceRecord(value)
  }

  return String(value)
}

function replacementForField(key: string): string | null {
  const normalized = normalizeFieldName(key)
  if (normalized === 'rawwindowtitle') {
    return '[omitted:raw-window-title]'
  }
  if (normalized === 'typedusertext') {
    return '[omitted:typed-user-text]'
  }
  if (normalized === 'rawlabel') {
    return '[omitted:raw-label]'
  }
  if (/(token|secret|password|credential|apikey|api_key|localauth)/i.test(key)) {
    return '[redacted:secret]'
  }
  return null
}

function violationForField(key: string, value: unknown): PrivacyPolicyViolation | null {
  if (typeof value === 'string' && (value.startsWith('[omitted:') || value.startsWith('[redacted:'))) {
    return null
  }

  const normalized = normalizeFieldName(key)
  if (normalized === 'rawwindowtitle') {
    return 'field.rawWindowTitle.omitted'
  }
  if (normalized === 'typedusertext') {
    return 'field.typedUserText.omitted'
  }
  if (normalized === 'rawlabel') {
    return 'field.rawLabel.omitted'
  }
  if (/(token|secret|password|credential|apikey|api_key|localauth)/i.test(key)) {
    return 'field.secret.redacted'
  }
  return null
}

function sanitizeUrl(value: string): string {
  try {
    const url = new URL(value)
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
      return '[redacted:url]'
    }
    if (!isLocalEvidenceOrigin(url)) {
      return `${url.origin}/[redacted-path]`
    }
    if (!isAllowedLocalEvidencePath(url.pathname)) {
      return `${url.origin}/[redacted-path]`
    }
    return `${url.origin}${url.pathname}`
  } catch {
    return value.replace(/([?&](?:token|api[_-]?key|password|secret)=)[^&#\s]+/gi, '$1[redacted]').replace(/#.*$/, '')
  }
}

function isLocalEvidenceOrigin(url: URL): boolean {
  return ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)
}

function isAllowedLocalEvidencePath(pathname: string): boolean {
  const localPaths = new Set<string>()
  for (const route of ALLOWED_WEB_VERIFICATION_ROUTES) {
    localPaths.add(route.path)
    localPaths.add(route.expectedPath)
  }
  return localPaths.has(pathname)
}

function isSafeStableIdentifier(value: string): boolean {
  return (
    value.length > 0 &&
    value.length <= 128 &&
    /^[A-Za-z0-9_.:-]+$/.test(value) &&
    !/(token|secret|password|api[_-]?key|credential|localauth)/i.test(value)
  )
}

function toHex32(value: number): string {
  return (value >>> 0).toString(16).padStart(8, '0')
}

function looksLikeUrl(value: string): boolean {
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(value)
}

function normalizeFieldName(key: string): string {
  return key.replace(/[_-]/g, '').toLowerCase()
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isJsonObject(value: JsonValue): value is { [key: string]: JsonValue } {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
