import { expect, test } from '@playwright/test'
import { mockBackgroundStreams, mockDefaultApiFallbacks } from './helpers/mock-api'

test.describe('Automation API fallback', () => {
  test('execute-hint endpoint works in mocked standalone fallback', async ({ page }) => {
    await mockBackgroundStreams(page)
    await mockDefaultApiFallbacks(page)

    await page.goto('/')

    const payload = {
      session_id: 'sess-e2e',
      intent_hint: 'click the save button',
    }

    const response = await page.evaluate(async (body) => {
      const res = await fetch('/api/automation/execute-hint', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      const json = await res.json()
      return { status: res.status, json }
    }, payload)

    expect(response.status).toBe(200)
    expect(response.json.command_id).toBeTruthy()
    expect(response.json.session_id).toBe('sess-e2e')
    expect(response.json.result.success).toBe(true)
  })
})
