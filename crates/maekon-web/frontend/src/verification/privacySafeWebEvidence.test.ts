import { describe, expect, it } from 'vitest'
import {
  ALLOWED_WEB_EVIDENCE_ARTIFACTS,
  ALLOWED_WEB_VERIFICATION_ROUTES,
  assertPrivacySafeArtifactRequest,
  buildRenderedStateManifest,
  findPrivacyPolicyViolations,
  redactEvidenceString,
  sanitizeEvidenceRecord,
  stableEvidenceToken,
} from './privacySafeWebEvidence'

describe('privacy-safe web evidence policy', () => {
  it('pins the first-pass local route and artifact allowlists', () => {
    expect(ALLOWED_WEB_VERIFICATION_ROUTES.map((route) => route.path)).toEqual([
      '/',
      '/timeline',
      '/reports',
      '/automation',
      '/settings',
    ])
    expect(
      ALLOWED_WEB_VERIFICATION_ROUTES.every(
        (route) =>
          'expectedContentSelector' in route &&
          typeof route.expectedContentSelector === 'string' &&
          route.expectedContentSelector.length > 0,
      ),
    ).toBe(true)
    expect(ALLOWED_WEB_EVIDENCE_ARTIFACTS).toEqual([
      'redacted_dom_snapshot',
      'cropped_region_screenshot',
      'console_summary',
      'network_summary',
      'geometry_summary',
      'evidence_manifest',
    ])
    expect(() => assertPrivacySafeArtifactRequest('broad_screenshot')).toThrow(/rejected/)
  })

  it('redacts URLs, credentials, raw titles, typed text, and raw labels', () => {
    const sanitized = sanitizeEvidenceRecord({
      url: 'http://127.0.0.1:5273/settings?token=abc123&api_key=def456#secret',
      local_auth_token: 'secret-token',
      rawWindowTitle: 'Jane Doe - Payroll - Maekon',
      typedUserText: 'hunter2',
      rawLabel: 'Pay Jane Doe now',
      stableId: 'settings.general',
      geometry: { x: 1, y: 2, width: 320, height: 240 },
    })

    expect(sanitized).toEqual({
      url: 'http://127.0.0.1:5273/settings',
      local_auth_token: '[redacted:secret]',
      rawWindowTitle: '[omitted:raw-window-title]',
      typedUserText: '[omitted:typed-user-text]',
      rawLabel: '[omitted:raw-label]',
      stableId: 'settings.general',
      geometry: { x: 1, y: 2, width: 320, height: 240 },
    })
    expect(redactEvidenceString('Authorization: Bearer abc.def.ghi')).toBe('Authorization: Bearer [redacted]')
  })

  it('redacts URL paths that can carry account tokens outside the local app origin', () => {
    const sanitized = sanitizeEvidenceRecord({
      externalUrl: 'https://example.com/accounts/user-secret-token-123?token=abc123#fragment',
      fileUrl: 'file:///Users/jane/.ssh/id_rsa',
      customUrl: 'tauri://localhost/settings/token-abc123',
      localRouteUrl: 'http://127.0.0.1:5273/settings/general?token=abc123',
    })

    expect(sanitized).toEqual({
      externalUrl: 'https://example.com/[redacted-path]',
      fileUrl: '[redacted:url]',
      customUrl: '[redacted:url]',
      localRouteUrl: 'http://127.0.0.1:5273/settings/general',
    })
  })

  it('hashes stable DOM identifiers instead of persisting raw locator strings', () => {
    const first = stableEvidenceToken('settings-save-floating')
    const second = stableEvidenceToken('settings-save-floating')

    expect(first).toMatch(/^stable:[a-f0-9]{16}$/)
    expect(first).toBe(second)
    expect(first).not.toContain('settings-save')
    expect(stableEvidenceToken('token=abc123')).toBe('[redacted:unstable-identifier]')
  })

  it('builds a failed rendered-state diagnostic bundle without unsafe evidence', () => {
    const manifest = buildRenderedStateManifest({
      route: '/automation',
      finalUrl: 'http://127.0.0.1:5273/automation?token=abc123',
      status: 'failed',
      checks: [
        {
          id: 'main-region-visible',
          passed: false,
          diagnosticCode: 'render.main_region_missing',
          rawLabel: 'Automation policy for Jane Doe',
        },
      ],
      artifacts: [
        {
          type: 'redacted_dom_snapshot',
          nodeCount: 3,
          nodes: [
            {
              tag: 'main',
              stableId: 'app-main',
              geometry: { x: 0, y: 0, width: 1024, height: 768 },
            },
          ],
        },
      ],
    })

    expect(manifest.status).toBe('failed')
    expect(manifest.finalUrl).toBe('http://127.0.0.1:5273/automation')
    expect(manifest.diagnosticBundle?.codes).toEqual(['render.main_region_missing'])
    expect(findPrivacyPolicyViolations(manifest)).toEqual([])
    expect(JSON.stringify(manifest)).not.toContain('Jane Doe')
    expect(JSON.stringify(manifest)).not.toContain('abc123')
  })

  it('reports policy violations for broad screenshots and raw evidence fields', () => {
    expect(
      findPrivacyPolicyViolations({
        artifacts: [{ type: 'broad_screenshot', path: '/tmp/full-desktop.png' }],
        rawWindowTitle: 'Secret Work',
      }),
    ).toEqual(['artifact.broad_screenshot.rejected', 'field.rawWindowTitle.omitted'])
  })
})
