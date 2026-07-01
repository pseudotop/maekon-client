import type { Page } from '@playwright/test'
import {
  type ALLOWED_WEB_VERIFICATION_ROUTES,
  buildRenderedStateManifest,
  findPrivacyPolicyViolations,
  type RenderedStateManifest,
  stableEvidenceToken,
} from '../../src/verification/privacySafeWebEvidence'

type WebVerificationRoute = (typeof ALLOWED_WEB_VERIFICATION_ROUTES)[number]

interface ConsoleSummary {
  [type: string]: number
}

interface NetworkResponseSummary {
  url: string
  method: string
  status: number
}

interface DomNodeSummary {
  tag: string
  role?: string
  stableToken?: string
  geometry: {
    x: number
    y: number
    width: number
    height: number
  }
}

export async function capturePrivacySafeRenderedEvidence(
  page: Page,
  route: WebVerificationRoute,
): Promise<RenderedStateManifest> {
  const consoleSummary: ConsoleSummary = {}
  const consoleErrors: string[] = []
  const pageErrors: string[] = []
  const networkResponses: NetworkResponseSummary[] = []

  page.on('console', (message) => {
    const type = message.type()
    consoleSummary[type] = (consoleSummary[type] ?? 0) + 1
    if (type === 'error' && consoleErrors.length < 10) {
      consoleErrors.push(message.text())
    }
  })
  page.on('pageerror', (error) => {
    if (pageErrors.length < 10) {
      pageErrors.push(error.message)
    }
  })
  page.on('response', (response) => {
    if (networkResponses.length >= 30) {
      return
    }
    networkResponses.push({
      url: response.url(),
      method: response.request().method(),
      status: response.status(),
    })
  })

  await page.goto(route.path)
  await page.waitForLoadState('domcontentloaded')
  await page.waitForLoadState('networkidle', { timeout: 5000 }).catch(() => undefined)

  const finalUrl = page.url()
  const finalPath = new URL(finalUrl).pathname
  const main = page.locator('main#main-content').first()
  const mainVisible = await main.isVisible().catch(() => false)
  const mainBox = mainVisible ? await main.boundingBox() : null
  const mainHasGeometry = Boolean(mainBox && mainBox.width > 0 && mainBox.height > 0)
  const expectedContent = page.locator(route.expectedContentSelector).first()
  const expectedContentVisible = await expectedContent.isVisible().catch(() => false)
  const routeFallbackVisible = await page
    .locator('main#main-content [data-testid="route-error-fallback"]')
    .first()
    .isVisible()
    .catch(() => false)
  const domNodes = await collectRedactedDomSnapshot(page)
  const routeMatched = finalPath === route.expectedPath
  const runtimeClean = consoleErrors.length === 0 && pageErrors.length === 0
  const rendered =
    mainVisible &&
    mainHasGeometry &&
    expectedContentVisible &&
    !routeFallbackVisible &&
    domNodes.length > 0 &&
    runtimeClean
  const status = routeMatched && rendered ? 'passed' : 'failed'

  const manifest = buildRenderedStateManifest({
    route: route.path,
    finalUrl,
    status,
    checks: [
      {
        id: 'route-redirect-complete',
        passed: routeMatched,
        diagnosticCode: 'render.expected_route_mismatch',
        expectedPath: route.expectedPath,
        finalPath,
      },
      {
        id: 'main-region-visible',
        passed: mainVisible,
        diagnosticCode: 'render.main_region_missing',
      },
      {
        id: 'main-region-has-geometry',
        passed: mainHasGeometry,
        diagnosticCode: 'render.main_region_empty_geometry',
        geometry: mainBox ? roundGeometry(mainBox) : null,
      },
      {
        id: 'redacted-dom-snapshot-nonempty',
        passed: domNodes.length > 0,
        diagnosticCode: 'render.dom_snapshot_empty',
      },
      {
        id: 'expected-route-content-visible',
        passed: expectedContentVisible,
        diagnosticCode: 'render.expected_content_missing',
        expectedContentSelector: route.expectedContentSelector,
      },
      {
        id: 'route-error-fallback-absent',
        passed: !routeFallbackVisible,
        diagnosticCode: 'render.route_error_fallback_visible',
      },
      {
        id: 'browser-runtime-clean',
        passed: runtimeClean,
        diagnosticCode: 'render.browser_runtime_error',
        consoleErrorCount: consoleErrors.length,
        pageErrorCount: pageErrors.length,
      },
    ],
    artifacts: [
      {
        type: 'geometry_summary',
        route: route.path,
        finalPath,
        mainRegion: mainBox ? roundGeometry(mainBox) : null,
      },
      {
        type: 'redacted_dom_snapshot',
        nodeCount: domNodes.length,
        nodes: domNodes,
      },
      {
        type: 'console_summary',
        counts: consoleSummary,
        errorCount: consoleErrors.length,
      },
      {
        type: 'network_summary',
        count: networkResponses.length,
        responses: networkResponses,
      },
      {
        type: 'evidence_manifest',
        generatedBy: 'privacy-safe-web-verification',
        policy: 'automation.gui.permission_evidence.v1',
      },
    ],
  })

  const violations = findPrivacyPolicyViolations(manifest)
  if (violations.length > 0) {
    throw new Error(`privacy-safe web verification policy violation: ${violations.join(', ')}`)
  }

  return manifest
}

async function collectRedactedDomSnapshot(page: Page): Promise<DomNodeSummary[]> {
  const rawNodes = await page.locator('main#main-content, nav, [data-testid], [role]').evaluateAll((elements) =>
    elements.slice(0, 40).map((element) => {
      const rect = element.getBoundingClientRect()
      return {
        tag: element.tagName.toLowerCase(),
        id: element.id || undefined,
        role: element.getAttribute('role') || undefined,
        stableId: element.getAttribute('data-testid') || undefined,
        geometry: {
          x: Math.round(rect.x),
          y: Math.round(rect.y),
          width: Math.round(rect.width),
          height: Math.round(rect.height),
        },
      }
    }),
  )

  return rawNodes.map(({ id, role, stableId, tag, geometry }) => {
    const stableToken = stableEvidenceToken(stableId ?? id ?? '')
    return {
      tag,
      role: role || undefined,
      stableToken: stableToken.startsWith('stable:') ? stableToken : undefined,
      geometry,
    }
  })
}

function roundGeometry(box: { x: number; y: number; width: number; height: number }) {
  return {
    x: Math.round(box.x),
    y: Math.round(box.y),
    width: Math.round(box.width),
    height: Math.round(box.height),
  }
}
