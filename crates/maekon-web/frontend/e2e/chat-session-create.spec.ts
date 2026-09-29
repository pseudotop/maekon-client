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
          if (cmd === 'get_feature_capabilities') {
            return Promise.resolve({
              features: [
                {
                  feature_id: 'provider_surface.openai.subprocess_cli',
                  maturity: 'stable',
                  availability: 'available',
                  provider_cli_readiness: 'invocation_ready',
                  provider_cli_discovery: {
                    candidate_name: 'Codex CLI',
                    executable_hint: 'codex',
                    version_status: 'not_checked',
                    dependency_status: 'ready',
                    status_reason: null,
                    env_refresh_required: false,
                  },
                  preferred: true,
                  requires: [],
                  status_reason: null,
                  status_copy_key: null,
                  setup_copy_key: null,
                  setup_docs_url: null,
                  configuration_env_vars: [],
                },
              ],
              ai_readiness: {
                contract_version: 1,
                capabilities: [
                  {
                    capability_id: 'chat.subprocess',
                    status: 'ready',
                    reason_code: 'ready',
                    action: 'none',
                    action_copy_key: 'aiReadiness.action.none',
                    dimensions: {
                      compiled_capability: true,
                      selected_access_mode: 'provider_subscription_cli',
                      access_mode_compatible: true,
                      endpoint_or_profile_configured: true,
                      provider_detection: 'detected',
                      provider_auth: 'ready',
                      provider_invocation: 'ready',
                      model_availability: 'not_required',
                      runtime_flag_enabled: true,
                      consent: [],
                      apply_requirement: 'runtime_applied',
                      apply_pending: false,
                      privacy_gate: 'enforced_at_invocation',
                      egress_gate: 'enforced_at_invocation',
                      budget_gate: 'enforced_at_invocation',
                      audit_gate: 'enforced_at_invocation',
                    },
                  },
                ],
              },
            })
          }
          // #9517: the real command is `get_token_usage` (the old
          // `get_token_usage_today` name never existed, so this mock was dead
          // and the real call fell through to the catch-all null). Payload
          // mirrors the current TokenUsageResponse camelCase contract
          // (model/provider added by #9466).
          if (cmd === 'get_token_usage') {
            return Promise.resolve({
              totalInputTokens: 0,
              totalOutputTokens: 0,
              dailyBudget: 10000,
              budgetRemaining: 10000,
              model: 'test-model',
              provider: 'anthropic',
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
    await expect(page.getByTestId('chat-provider-readiness')).toHaveAttribute('data-transport', 'subprocess')
    await expect(page.getByTestId('chat-provider-readiness')).toContainText('Codex CLI')

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

    // The button is now labeled "Loading..." and must be disabled, which is
    // the user-observable duplicate-create guard. Playwright intentionally
    // refuses a normal click on disabled controls, so asserting disabled is
    // the faithful interaction check rather than waiting for an impossible
    // second click.
    const loadingBtn = page.getByRole('button', { name: loadingText })
    await expect(loadingBtn).toBeVisible({ timeout: 2000 })
    await expect(loadingBtn).toBeDisabled()

    // Wait for the creation to complete
    await expect(page.locator('form textarea')).toBeVisible({ timeout: 5000 })

    // Verify only one create_ai_session call was made
    const count = await page.evaluate(() => (globalThis as Record<string, unknown>).__CREATE_CALL_COUNT__)
    expect(count).toBe(1)
  })
})
