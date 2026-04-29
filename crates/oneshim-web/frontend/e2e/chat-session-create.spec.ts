/**
 * E2E tests for the Chat page session creation flow via EmptyState CTA.
 *
 * These tests use __TAURI_INTERNALS__ mock to simulate Tauri IPC so the
 * create_ai_session command actually succeeds and the UI transitions from
 * empty state → active session with input area.
 */

import { i18nRegex } from './helpers/i18n'
import { mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const emptyChatTitle = i18nRegex('emptyState.chat.title')
const emptyChatAction = i18nRegex('emptyState.chat.action')
const loadingText = i18nRegex('common.loading')

function mockTauriIpc(page: Page, opts?: { createDelay?: number }) {
  return page.addInitScript(
    ([delay]) => {
      let createCallCount = 0
      ;(globalThis as Record<string, unknown>).__TAURI_INTERNALS__ = {
        invoke: (cmd: string, args?: Record<string, unknown>) => {
          if (cmd === 'get_onboarding_status') {
            return Promise.resolve({ completed: true })
          }
          if (cmd === 'list_ai_sessions') {
            return Promise.resolve([])
          }
          if (cmd === 'get_token_usage_today') {
            return Promise.resolve({
              totalInputTokens: 0,
              totalOutputTokens: 0,
              dailyBudget: 10000,
              budgetRemaining: 10000,
            })
          }
          if (cmd === 'create_ai_session') {
            createCallCount++
            ;(globalThis as Record<string, unknown>).__CREATE_CALL_COUNT__ = createCallCount
            const session = {
              session_id: `test-sess-${createCallCount}`,
              provider_name: 'test-provider',
              model: 'test-model',
              state: 'active',
              transport: (args?.config as Record<string, unknown>)?.transport || 'subprocess',
              created_at: '2026-04-09T00:00:00Z',
              last_active: '2026-04-09T00:00:00Z',
              turn_count: 0,
              title: null,
            }
            if (delay > 0) {
              return new Promise((resolve) => setTimeout(() => resolve(session), delay))
            }
            return Promise.resolve(session)
          }
          if (cmd === 'load_session_messages') {
            return Promise.resolve([])
          }
          // Catch-all for event listeners, audio, etc.
          return Promise.resolve(null)
        },
      }
    },
    [opts?.createDelay ?? 0],
  )
}

async function mockChatApis(page: Page) {
  await mockStaticJson(page, '**/api/ai/provider-surfaces', {
    version: 1,
    updated_at: '2026-04-09T00:00:00Z',
    vendors: [],
    surfaces: [],
  })
}

test.describe('Chat session creation via EmptyState CTA', () => {
  test('clicking New Session creates a session and shows the message input', async ({ page }) => {
    await mockTauriIpc(page)
    await mockChatApis(page)

    await page.goto('/chat')
    await expect(page.getByRole('heading', { name: emptyChatTitle })).toBeVisible({ timeout: 10000 })

    // Click the CTA button
    await page.getByRole('button', { name: emptyChatAction }).click()

    // After session creation, the message textarea should appear
    const textarea = page.locator('form textarea')
    await expect(textarea).toBeVisible({ timeout: 5000 })

    // The empty state heading should be gone
    await expect(page.getByRole('heading', { name: emptyChatTitle })).not.toBeVisible()
  })

  test('CTA shows loading state during session creation', async ({ page }) => {
    // Add 500ms delay so we can observe the loading label
    await mockTauriIpc(page, { createDelay: 500 })
    await mockChatApis(page)

    await page.goto('/chat')
    await expect(page.getByRole('heading', { name: emptyChatTitle })).toBeVisible({ timeout: 10000 })

    await page.getByRole('button', { name: emptyChatAction }).click()

    // Button label should briefly show loading text
    await expect(page.getByRole('button', { name: loadingText })).toBeVisible({ timeout: 2000 })

    // After creation completes, textarea appears
    await expect(page.locator('form textarea')).toBeVisible({ timeout: 5000 })
  })

  test('rapid double-click creates only one session (guard)', async ({ page }) => {
    // 1-second delay to keep the creating state active during double-click
    await mockTauriIpc(page, { createDelay: 1000 })
    await mockChatApis(page)

    await page.goto('/chat')
    await expect(page.getByRole('heading', { name: emptyChatTitle })).toBeVisible({ timeout: 10000 })

    // First click changes the label from "New Session" to "Loading..."
    await page.getByRole('button', { name: emptyChatAction }).click()

    // The button is now labeled "Loading..." — try clicking it again
    const loadingBtn = page.getByRole('button', { name: loadingText })
    await expect(loadingBtn).toBeVisible({ timeout: 2000 })
    await loadingBtn.click()

    // Wait for the creation to complete
    await expect(page.locator('form textarea')).toBeVisible({ timeout: 5000 })

    // Verify only one create_ai_session call was made
    const count = await page.evaluate(() => (globalThis as Record<string, unknown>).__CREATE_CALL_COUNT__)
    expect(count).toBe(1)
  })
})
