/**
 * Onboarding E2E tests.
 *
 * The Onboarding component is not routable — it is shown conditionally when
 * `get_onboarding_status` returns `{ completed: false }` in the Tauri runtime.
 * In standalone/dev mode (no Tauri), the IPC call fails and onboarding is skipped.
 *
 * To test Onboarding in E2E we bypass the Tauri check by rendering the page at
 * "/" with localStorage flag that simulates first-run, but since the standalone
 * fallback always sets onboardingDone=true, we instead directly navigate to
 * a test-only harness or verify the component renders when the shell cannot load.
 *
 * Approach: We intercept the Tauri IPC bootstrap so the Onboarding component
 * never appears (standalone mode always skips it). These tests therefore verify
 * the component's DOM structure via direct page load with mocked module state.
 * Since the onboarding gate lives inside a dynamic import(`@tauri-apps/api/core`)
 * catch block, the spec simply verifies the main shell loads instead.
 *
 * NOTE: Full onboarding flow testing requires a Tauri integration test context.
 * This spec documents the coverage gap and tests what is reachable in pure web E2E.
 */

import { i18nRegex } from './helpers/i18n'
import { expect, test } from './helpers/test'

const dashboardTitleName = i18nRegex('dashboard.title', ['Dashboard preparing'])
const step1TitleName = i18nRegex('onboarding.step1Title')
const step2TitleName = i18nRegex('onboarding.step2Title')
const step4TitleName = i18nRegex('onboarding.step4Title')
const nextButtonName = i18nRegex('onboarding.next')
const skipButtonName = i18nRegex('onboarding.skip')
const completeButtonName = i18nRegex('onboarding.complete')
const step4TipName = /⌘K|Ctrl\+K/

test.describe('Onboarding (standalone mode)', () => {
  test('standalone mode skips onboarding and shows dashboard', async ({ page }) => {
    // In standalone/dev mode, get_onboarding_status IPC fails,
    // so the app skips onboarding and shows the main shell directly.
    await page.goto('/')
    await expect(page.getByRole('heading', { name: dashboardTitleName })).toBeVisible({ timeout: 10000 })

    // Onboarding step titles should NOT be visible
    await expect(page.getByText(step1TitleName)).not.toBeVisible()
  })

  test('onboarding i18n keys exist for all steps', async ({ page }) => {
    // Verify i18n keys resolve without throwing (validation at build time).
    // i18nRegex would throw if a key is missing from en.json or ko.json.
    expect(step1TitleName).toBeTruthy()
    expect(step2TitleName).toBeTruthy()
    expect(step4TitleName).toBeTruthy()
    expect(nextButtonName).toBeTruthy()
    expect(skipButtonName).toBeTruthy()
    expect(completeButtonName).toBeTruthy()
  })
})

test.describe('Onboarding (simulated first-run)', () => {
  // Force onboarding to show by intercepting the Tauri module import
  // so that the dynamic import resolves with completed: false.
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => {
      // Patch globalThis so the dynamic import('@tauri-apps/api/core') resolves
      // with a fake invoke. Anything we don't explicitly stub resolves with
      // `null` so that when the user clicks Skip, AppShell can mount its
      // useTauriEventBridge → listen() chain without rejecting on unrelated
      // Tauri commands like `plugin:event|listen` (an earlier version
      // rejected and the unhandled error fell into ErrorBoundary, hiding
      // the dashboard heading the spec is asserting on).
      ;(globalThis as Record<string, unknown>).__TAURI_INTERNALS__ = {
        invoke: (cmd: string) => {
          if (cmd === 'get_onboarding_status') {
            return Promise.resolve({ completed: false })
          }
          if (cmd === 'complete_onboarding') {
            return Promise.resolve()
          }
          return Promise.resolve(null)
        },
      }
    })
  })

  test('should display step 1 intro on first run', async ({ page }) => {
    await page.goto('/')
    await expect(page.getByText(step1TitleName)).toBeVisible({ timeout: 10000 })
  })

  test('should show next and skip buttons', async ({ page }) => {
    await page.goto('/')
    await expect(page.getByText(step1TitleName)).toBeVisible({ timeout: 10000 })
    await expect(page.getByRole('button', { name: nextButtonName })).toBeVisible()
    await expect(page.getByRole('button', { name: skipButtonName })).toBeVisible()
  })

  test('should navigate to permissions step', async ({ page }) => {
    await page.goto('/')
    await expect(page.getByText(step1TitleName)).toBeVisible({ timeout: 10000 })

    await page.getByRole('button', { name: nextButtonName }).click()
    await expect(page.getByText(step2TitleName)).toBeVisible({ timeout: 5000 })
  })

  test('should show step indicator dots', async ({ page }) => {
    await page.goto('/')
    await expect(page.getByText(step1TitleName)).toBeVisible({ timeout: 10000 })

    const stepIndicator = page.locator('fieldset[aria-label="Step indicator"]')
    await expect(stepIndicator).toBeVisible()
    // 4 dots for 4 steps
    const dots = stepIndicator.locator('div')
    await expect(dots).toHaveCount(4)
  })

  test('should skip onboarding via skip button', async ({ page }) => {
    await page.goto('/')
    await expect(page.getByText(step1TitleName)).toBeVisible({ timeout: 10000 })

    await page.getByRole('button', { name: skipButtonName }).click()
    // After skip, the main shell should load
    await expect(page.getByRole('heading', { name: dashboardTitleName })).toBeVisible({ timeout: 10000 })
  })

  test('step 4 shows CommandPalette keyboard shortcut tip', async ({ page }) => {
    await page.goto('/')
    await expect(page.getByText(step1TitleName)).toBeVisible({ timeout: 10000 })

    // Advance to step 4 (step 1 → 2 → 3 → 4)
    const nextBtn = page.getByRole('button', { name: nextButtonName })
    await nextBtn.click()
    await expect(page.getByText(step2TitleName)).toBeVisible({ timeout: 5000 })
    await nextBtn.click()
    // step 3 (features)
    await nextBtn.click()
    // step 4 (ready)
    await expect(page.getByText(step4TitleName)).toBeVisible({ timeout: 5000 })

    // The tip card with role="note" should mention the keyboard shortcut
    const tipNote = page.locator('[role="note"]')
    await expect(tipNote).toBeVisible()
    await expect(tipNote).toContainText(step4TipName)
  })
})
