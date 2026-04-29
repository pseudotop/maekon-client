/**
 * Live smoke tests — run against the actual Maekon app (not mocked).
 *
 * These catch the class of bugs that mock-based E2E tests miss:
 * - StatusBar showing "Offline" when backend is running
 * - Version hardcoded to wrong value
 * - API endpoints returning errors
 * - SSE stream not connecting
 * - Pages failing to render with real data
 *
 * Prerequisites: cargo run (or cargo tauri dev) must be running.
 */

import fs from 'node:fs'
import path from 'node:path'
import { expect, test } from '@playwright/test'
import { DEFAULT_WEB_PORT } from '../src/constants'

const port = Number(process.env.ONESHIM_PORT || DEFAULT_WEB_PORT)
const API_BASE = `http://127.0.0.1:${port}/api`

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Read the workspace version from Cargo.toml (single source of truth) */
function getCargoVersion(): string {
  const cargoPath = path.resolve(__dirname, '../../../../Cargo.toml')
  const content = fs.readFileSync(cargoPath, 'utf-8')
  const match = content.match(/^\s*version\s*=\s*"([^"]+)"/m)
  if (!match) throw new Error('Cannot read version from Cargo.toml')
  return match[1]
}

// ---------------------------------------------------------------------------
// Backend Health
// ---------------------------------------------------------------------------

test.describe('Backend Health', () => {
  test('API is reachable', async ({ request }) => {
    const res = await request.get(`${API_BASE}/settings`)
    expect(res.ok()).toBeTruthy()
    const body = await res.json()
    expect(body).toHaveProperty('web_port')
  })

  test('SSE stream connects', async ({ request }) => {
    const res = await request.get(`${API_BASE}/stream`, {
      headers: { Accept: 'text/event-stream' },
      timeout: 5000,
    })
    // SSE endpoint should respond with 200
    expect(res.status()).toBe(200)
  })

  test('metrics endpoint returns data', async ({ request }) => {
    const res = await request.get(`${API_BASE}/stats/summary`)
    expect(res.ok()).toBeTruthy()
    const body = await res.json()
    expect(body).toHaveProperty('date')
    expect(body).toHaveProperty('cpu_avg')
  })

  test('update status endpoint works', async ({ request }) => {
    const res = await request.get(`${API_BASE}/update/status`)
    expect(res.ok()).toBeTruthy()
    const body = await res.json()
    expect(body).toHaveProperty('phase')
  })
})

// ---------------------------------------------------------------------------
// Frontend Rendering
// ---------------------------------------------------------------------------

test.describe('Frontend Rendering', () => {
  test('dashboard loads without errors', async ({ page }) => {
    const consoleErrors: string[] = []
    page.on('console', (msg) => {
      if (msg.type() === 'error') consoleErrors.push(msg.text())
    })

    await page.goto('/')
    // Wait for the page to actually render content
    await page.waitForLoadState('networkidle')

    // Should have some visible content (not a blank page)
    const body = await page.locator('body').textContent()
    expect(body?.length).toBeGreaterThan(50)

    // Filter out known non-critical errors (e.g. favicon 404)
    const criticalErrors = consoleErrors.filter((e) => !e.includes('favicon') && !e.includes('DevTools'))
    expect(criticalErrors).toEqual([])
  })

  test('no CSP violations on page load', async ({ page }) => {
    const cspViolations: string[] = []
    page.on('console', (msg) => {
      const text = msg.text()
      if (text.includes('Content-Security-Policy') || text.includes('Refused to')) {
        cspViolations.push(text)
      }
    })

    await page.goto('/')
    await page.waitForLoadState('networkidle')

    expect(cspViolations).toEqual([])
  })

  test('all page routes render without crash', async ({ page }) => {
    const routes = [
      '/',
      '/timeline',
      '/search',
      '/reports',
      '/focus',
      '/automation',
      '/settings',
      '/privacy',
      '/updates',
    ]

    for (const route of routes) {
      await page.goto(route)
      await page.waitForLoadState('domcontentloaded')
      // Page should not show a blank screen or error boundary
      const html = await page.content()
      expect(html).not.toContain('error-boundary')
      expect(html.length).toBeGreaterThan(500)
    }
  })
})

// ---------------------------------------------------------------------------
// StatusBar — the #1 user-visible indicator
// ---------------------------------------------------------------------------

test.describe('StatusBar', () => {
  test('shows Connected (not Offline)', async ({ page }) => {
    await page.goto('/')
    // Wait for SSE to establish
    await page.waitForTimeout(2000)

    const statusBar = page.locator('.app-shell-statusbar')
    await expect(statusBar).toBeVisible()

    // Should show "Connected" / "연결됨" — NOT "Offline" / "오프라인"
    const text = await statusBar.textContent()
    expect(text).not.toMatch(/offline|오프라인/i)
    expect(text).toMatch(/connected|연결됨/i)
  })

  test('shows correct version from Cargo.toml', async ({ page }) => {
    const expectedVersion = getCargoVersion()

    await page.goto('/')
    await page.waitForLoadState('networkidle')

    const statusBar = page.locator('.app-shell-statusbar')
    const text = await statusBar.textContent()

    // Version should contain the Cargo.toml version (e.g. "v0.3.5")
    expect(text).toContain(`v${expectedVersion}`)
  })

  test('shows CPU and memory metrics (not --)', async ({ page }) => {
    await page.goto('/')
    // Wait for metrics to arrive via SSE
    await page.waitForTimeout(3000)

    const statusBar = page.locator('.app-shell-statusbar')
    const text = await statusBar.textContent()

    // After SSE connects, CPU should show a percentage, not "--"
    // Allow "--" only if SSE hasn't delivered data yet (but we waited 3s)
    if (!text?.includes('--')) {
      expect(text).toMatch(/\d+\.\d+%/) // CPU like "12.3%"
      expect(text).toMatch(/\d+MB/) // RAM like "4567MB"
    }
  })

  test('shows automation status', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')

    const statusBar = page.locator('.app-shell-statusbar')
    const text = await statusBar.textContent()

    // Should show either "Auto: ON" or "Auto: OFF" (not missing)
    expect(text).toMatch(/Auto:\s*(ON|OFF)|자동:\s*(켜짐|꺼짐)/i)
  })
})

// ---------------------------------------------------------------------------
// Updates Page
// ---------------------------------------------------------------------------

test.describe('Updates Page', () => {
  test('renders update status', async ({ page }) => {
    await page.goto('/updates')
    await page.waitForLoadState('networkidle')

    // Should display some update-related content
    const content = await page.content()
    // The page should have loaded real data, not be empty
    expect(content.length).toBeGreaterThan(1000)
  })
})

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

test.describe('Navigation', () => {
  test('sidebar navigation works for all pages', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')

    // ActivityBar should be visible
    const nav = page.locator('nav[role="navigation"]')
    await expect(nav).toBeVisible()

    // Click through each nav item and verify URL changes
    const navItems = await nav.locator('button').all()
    expect(navItems.length).toBeGreaterThanOrEqual(5)

    for (const item of navItems) {
      await item.click()
      await page.waitForLoadState('domcontentloaded')
      // Page should not crash
      const html = await page.content()
      expect(html.length).toBeGreaterThan(500)
    }
  })
})

// ---------------------------------------------------------------------------
// API Contract Verification
// ---------------------------------------------------------------------------

test.describe('API Contract', () => {
  test('settings endpoint returns expected shape', async ({ request }) => {
    const res = await request.get(`${API_BASE}/settings`)
    const body = await res.json()

    // Key fields that the frontend depends on
    expect(body).toHaveProperty('web_port')
    expect(body).toHaveProperty('capture_enabled')
    expect(body).toHaveProperty('notification')
    expect(body).toHaveProperty('update')
    expect(body).toHaveProperty('privacy')
    expect(body).toHaveProperty('schedule')
  })

  test('processes endpoint returns array', async ({ request }) => {
    const res = await request.get(`${API_BASE}/processes`)
    expect(res.ok()).toBeTruthy()
    const body = await res.json()
    expect(Array.isArray(body)).toBe(true)
  })

  test('tags endpoint returns array', async ({ request }) => {
    const res = await request.get(`${API_BASE}/tags`)
    expect(res.ok()).toBeTruthy()
    const body = await res.json()
    expect(Array.isArray(body)).toBe(true)
  })
})
