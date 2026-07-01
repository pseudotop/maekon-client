import {
  ALLOWED_WEB_VERIFICATION_ROUTES,
  assertPrivacySafeArtifactRequest,
} from '../src/verification/privacySafeWebEvidence'
import { capturePrivacySafeRenderedEvidence } from './helpers/privacy-safe-web-verification'
import { expect, test } from './helpers/test'

test.use({ trace: 'off', screenshot: 'off', video: 'off' })

test.describe('privacy-safe rendered web verification harness', () => {
  for (const route of ALLOWED_WEB_VERIFICATION_ROUTES) {
    test(`proves rendered state for ${route.path}`, async ({ page }, testInfo) => {
      const manifest = await capturePrivacySafeRenderedEvidence(page, route)

      await testInfo.attach(`privacy-safe-web-evidence-${route.path.replace(/\W+/g, '_') || 'root'}`, {
        body: JSON.stringify(manifest, null, 2),
        contentType: 'application/json',
      })

      expect(manifest.status, JSON.stringify(manifest.checks, null, 2)).toBe('passed')
      expect(manifest.finalUrl).not.toContain('?')
      expect(manifest.diagnosticBundle).toBeUndefined()
      expect(JSON.stringify(manifest)).not.toContain('rawWindowTitle')
      expect(JSON.stringify(manifest)).not.toContain('typedUserText')
      expect(JSON.stringify(manifest)).not.toContain('data-testid')
      expect(JSON.stringify(manifest)).not.toContain('settings-save')
      expect(JSON.stringify(manifest)).not.toContain('frame-card-')
    })
  }

  test('rejects broad screenshot artifacts before browser capture', () => {
    expect(() => assertPrivacySafeArtifactRequest('broad_screenshot')).toThrow(/rejected/)
  })
})
