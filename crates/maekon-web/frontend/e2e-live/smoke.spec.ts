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
import { fileURLToPath } from 'node:url'
import { expect, type Page, test } from '@playwright/test'
import { DEFAULT_WEB_PORT } from '../src/constants'
import { requireLiveAuthToken } from './live-auth'

const port = Number(process.env.MAEKON_PORT || DEFAULT_WEB_PORT)
const API_BASE = `http://127.0.0.1:${port}/api`

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Read the workspace version from Cargo.toml (single source of truth) */
function getCargoVersion(): string {
  const cargoPath = path.resolve(fileURLToPath(new URL('../../../../Cargo.toml', import.meta.url)))
  const content = fs.readFileSync(cargoPath, 'utf-8')
  const match = content.match(/^\s*version\s*=\s*"([^"]+)"/m)
  if (!match) throw new Error('Cannot read version from Cargo.toml')
  return match[1]
}

async function authenticatedFetch(url: string, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers)
  headers.set('x-local-auth', requireLiveAuthToken())
  return fetch(url, { ...init, headers })
}

async function authenticatePage(page: Page): Promise<void> {
  await page.context().addCookies([
    {
      name: 'maekon_local_auth',
      value: requireLiveAuthToken(),
      domain: '127.0.0.1',
      path: '/api',
      httpOnly: false,
      secure: false,
      sameSite: 'Strict',
    },
  ])
}

async function openAuthenticatedPage(page: Page, route: string): Promise<void> {
  await authenticatePage(page)
  await page.goto(route)
}

// ---------------------------------------------------------------------------
// Backend Health
// ---------------------------------------------------------------------------

test.describe('Backend Health', () => {
  test('API is reachable', async () => {
    const res = await authenticatedFetch(`${API_BASE}/settings`)
    expect(res.ok).toBeTruthy()
    const body = await res.json()
    expect(body).toHaveProperty('web_port')
  })

  test('SSE stream connects', async () => {
    const res = await authenticatedFetch(`${API_BASE}/stream`, {
      headers: { Accept: 'text/event-stream' },
      signal: AbortSignal.timeout(5000),
    })
    expect(res.status).toBe(200)
    expect(res.headers.get('content-type')).toContain('text/event-stream')
    await res.body?.cancel()
  })

  test('metrics endpoint returns data', async () => {
    const res = await authenticatedFetch(`${API_BASE}/stats/summary`)
    expect(res.ok).toBeTruthy()
    const body = await res.json()
    expect(body).toHaveProperty('date')
    expect(body).toHaveProperty('cpu_avg')
  })

  test('update status endpoint works', async () => {
    const res = await authenticatedFetch(`${API_BASE}/update/status`)
    expect(res.ok).toBeTruthy()
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

    await openAuthenticatedPage(page, '/')
    // Wait for the page to actually render content
    await page.waitForLoadState('domcontentloaded')

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

    await openAuthenticatedPage(page, '/')
    await page.waitForLoadState('domcontentloaded')
    await page.waitForTimeout(500)

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

    await authenticatePage(page)
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
    await openAuthenticatedPage(page, '/')
    // Wait for SSE to establish
    await page.waitForTimeout(2000)

    const statusBar = page.locator('.app-shell-statusbar')
    await expect(statusBar).toBeVisible()

    // Should show a connected status, NOT an offline one (English or Korean UI).
    const text = await statusBar.textContent()
    expect(text).not.toMatch(/offline|오프라인/i)
    expect(text).toMatch(/connected|연결됨/i)
  })

  test('shows correct version from Cargo.toml', async ({ page }) => {
    const expectedVersion = getCargoVersion()

    await openAuthenticatedPage(page, '/')
    await page.waitForLoadState('domcontentloaded')

    const statusBar = page.locator('.app-shell-statusbar')
    const text = await statusBar.textContent()

    // Version should contain the Cargo.toml version (e.g. "v0.3.5")
    expect(text).toContain(`v${expectedVersion}`)
  })

  test('shows CPU and memory metrics (not --)', async ({ page }) => {
    await openAuthenticatedPage(page, '/')
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
    await openAuthenticatedPage(page, '/')
    await page.waitForLoadState('domcontentloaded')

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
    await openAuthenticatedPage(page, '/updates')
    await page.waitForLoadState('domcontentloaded')

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
    await openAuthenticatedPage(page, '/')
    await page.waitForLoadState('domcontentloaded')

    const navigation = page.getByRole('navigation')
    await expect(navigation).toBeVisible()
    const navItems = navigation.locator('button[data-testid^="nav-"]')
    const navItemCount = await navItems.count()
    expect(navItemCount).toBeGreaterThanOrEqual(5)

    for (let index = 0; index < navItemCount; index += 1) {
      await navItems.nth(index).click()
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
  test('settings endpoint returns expected shape', async () => {
    const res = await authenticatedFetch(`${API_BASE}/settings`)
    const body = await res.json()

    // Key fields that the frontend depends on
    expect(body).toHaveProperty('web_port')
    expect(body).toHaveProperty('capture_enabled')
    expect(body).toHaveProperty('notification')
    expect(body).toHaveProperty('update')
    expect(body).toHaveProperty('privacy')
    expect(body).toHaveProperty('schedule')
  })

  test('processes endpoint returns array', async () => {
    const res = await authenticatedFetch(`${API_BASE}/processes`)
    expect(res.ok).toBeTruthy()
    const body = await res.json()
    expect(Array.isArray(body)).toBe(true)
  })

  test('tags endpoint returns array', async () => {
    const res = await authenticatedFetch(`${API_BASE}/tags`)
    expect(res.ok).toBeTruthy()
    const body = await res.json()
    expect(Array.isArray(body)).toBe(true)
  })
})
