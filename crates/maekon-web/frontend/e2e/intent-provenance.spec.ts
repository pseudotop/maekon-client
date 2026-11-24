// Synthetic browser contract only: no provider, model or native action is invoked.
import {
  EXECUTION_A,
  EXECUTION_B,
  makeFallback,
  makeProvenance,
  makeResponse,
  PRIVATE_CANARY,
} from '../src/api/__tests__/fixtures/intentExecution'
import type { ExecuteIntentHintRequest } from '../src/api/contracts'
import { expect, type Route, test } from './helpers/test'

const SYNTHETIC_AUTH = 'synthetic-intent-browser-token'
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

test.beforeEach(async ({ page }) => {
  await page.addInitScript((token) => {
    localStorage.setItem('maekon-language', 'en')
    const injectedWindow = window as Window & { __MAEKON_LOCAL_AUTH__?: string }
    injectedWindow.__MAEKON_LOCAL_AUTH__ = token
  }, SYNTHETIC_AUTH)
})

test('lost response is recovered by exact authenticated observation; explicit retry gets a fresh identity', async ({
  page,
}) => {
  const posts: ExecuteIntentHintRequest[] = []
  const queries: string[] = []
  await page.route('**/api/automation/execute-hint', async (route) => {
    expect(route.request().method()).toBe('POST')
    expect(route.request().headers()['x-local-auth']).toBe(SYNTHETIC_AUTH)
    const payload = route.request().postDataJSON() as ExecuteIntentHintRequest
    posts.push(payload)
    if (posts.length === 1) return route.abort('connectionreset')
    await route.fulfill({
      json: makeResponse(makeProvenance({ submission_id: payload.submission_id, execution_id: EXECUTION_B })),
    })
  })
  await page.route('**/api/automation/audit?submission_id=*', async (route) => {
    expect(route.request().method()).toBe('GET')
    expect(route.request().headers()['x-local-auth']).toBe(SYNTHETIC_AUTH)
    const query = new URL(route.request().url()).searchParams
    expect([...query.keys()]).toEqual(['submission_id'])
    queries.push(query.get('submission_id') ?? '')
    const p = { ...makeFallback(), submission_id: posts[0].submission_id }
    // Transport order is not observation order.
    p.events.reverse()
    await route.fulfill({ json: [{ execution_provenance: p }] })
  })
  await page.goto('/automation/policies')
  await page.getByTestId('intent-hint-input').fill('Save the synthetic document')
  await page.getByTestId('intent-hint-run').click()
  await expect(page.getByTestId('intent-provenance-unknown')).toBeVisible()
  await expect(page.getByText('Running again starts a new action and may duplicate the earlier one.')).toBeVisible()
  expect(posts).toHaveLength(1)
  expect(posts[0].submission_id).toMatch(UUID)
  expect(posts[0].retry_of).toBeUndefined()
  await page.getByTestId('intent-observe-current').click()
  await expect(page.getByTestId('intent-observed-plan')).toHaveText('Rule-based plan')
  const attempts = page.getByTestId('intent-provenance-attempt')
  await expect(attempts).toHaveCount(2)
  await expect(attempts.nth(0)).toContainText('Remote model — Unavailable')
  await expect(attempts.nth(1)).toContainText('This route does not support skills')
  await expect(page.getByText('Matching execution observed. No action was sent.')).toBeVisible()
  await page.getByText('Observed steps', { exact: true }).click()
  expect(
    await page
      .getByTestId('intent-observed-event')
      .evaluateAll((nodes) => nodes.map((node) => Number(node.getAttribute('data-sequence')))),
  ).toEqual([0, 1, 2, 3, 4, 5, 6, 7])
  expect(queries).toEqual([posts[0].submission_id])
  expect(posts).toHaveLength(1)
  await page.getByTestId('intent-hint-run').click()
  await expect(page.getByTestId('intent-observed-plan')).toHaveText('Model-based plan')
  expect(posts).toHaveLength(2)
  expect(posts[1].submission_id).toMatch(UUID)
  expect(posts[1].submission_id).not.toBe(posts[0].submission_id)
  expect(posts[1].retry_of).toBe(EXECUTION_A)
})

test('ambiguous observation stays unknown and private error text is never rendered', async ({ page }) => {
  const posts: ExecuteIntentHintRequest[] = []
  await page.route('**/api/automation/execute-hint', async (route) => {
    posts.push(route.request().postDataJSON() as ExecuteIntentHintRequest)
    await route.fulfill({ status: 503, json: { error: PRIVATE_CANARY } })
  })
  await page.route('**/api/automation/audit?submission_id=*', async (route) => {
    const p = makeProvenance({ submission_id: posts[0].submission_id })
    await route.fulfill({ json: [{ execution_provenance: p }, { execution_provenance: p }] })
  })
  await page.goto('/automation/policies')
  await page.getByTestId('intent-hint-input').fill('Synthetic unavailable action')
  await page.getByTestId('intent-hint-run').click()
  await expect(page.getByTestId('intent-provenance-unknown')).toBeVisible()
  await page.getByTestId('intent-observe-current').click()
  await expect(
    page.getByText('No unique matching observation is available. This does not prove the action did not run.'),
  ).toBeVisible()
  await expect(page.getByTestId('intent-provenance-unknown')).toBeVisible()
  await expect(page.locator('body')).not.toContainText('PRIVATE_CANARY')
  expect(posts).toHaveLength(1)
})

test('a late observation for submission A cannot replace submission B', async ({ page }) => {
  const posts: ExecuteIntentHintRequest[] = []
  let captured: Route | undefined
  await page.route('**/api/automation/execute-hint', async (route) => {
    const payload = route.request().postDataJSON() as ExecuteIntentHintRequest
    posts.push(payload)
    if (posts.length === 1) return route.abort('connectionreset')
    await route.fulfill({
      status: 403,
      json: {
        error: 'Synthetic denial',
        execution_provenance: makeProvenance({
          submission_id: payload.submission_id,
          execution_id: EXECUTION_B,
          planning_outcome: 'denied',
          action_outcome: 'denied',
          attempts: [],
        }),
      },
    })
  })
  await page.route('**/api/automation/audit?submission_id=*', async (route) => {
    captured = route
  })
  await page.goto('/automation/policies')
  await page.getByTestId('intent-hint-input').fill('Synthetic request A')
  await page.getByTestId('intent-hint-run').click()
  await expect(page.getByTestId('intent-provenance-unknown')).toBeVisible()
  await page.getByTestId('intent-observe-current').click()
  await expect.poll(() => Boolean(captured)).toBe(true)
  await page.getByTestId('intent-hint-input').fill('Synthetic request B')
  await page.getByTestId('intent-hint-run').click()
  await expect(page.getByTestId('intent-observed-plan')).toHaveText('Planning denied')
  expect(posts).toHaveLength(2)
  expect(posts[1].retry_of).toBeUndefined()
  const response = page.waitForResponse('**/api/automation/audit?submission_id=*')
  if (!captured) throw new Error('Observation request was not captured')
  await captured.fulfill({
    json: [{ execution_provenance: { ...makeFallback(), submission_id: posts[0].submission_id } }],
  })
  await response
  await page.evaluate(
    () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
  )
  await expect(page.getByTestId('intent-observed-plan')).toHaveText('Planning denied')
  await expect(page.getByTestId('intent-provenance-attempt')).toHaveCount(0)
  await expect(page.getByText('Matching execution observed. No action was sent.')).toHaveCount(0)
  expect(posts).toHaveLength(2)
})
