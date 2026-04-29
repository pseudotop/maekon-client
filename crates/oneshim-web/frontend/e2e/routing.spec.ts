import { expect, test } from './helpers/test'

test.describe('Sub-pathname routing redirects', () => {
  test('/ redirects to /overview', async ({ page }) => {
    await page.goto('/')
    await page.waitForURL('**/overview')
    await expect(page).toHaveURL(/\/overview$/)
  })

  test('/settings redirects to /settings/general', async ({ page }) => {
    await page.goto('/settings')
    await page.waitForURL('**/settings/general')
    await expect(page).toHaveURL(/\/settings\/general$/)
  })

  test('/automation redirects to /automation/policies', async ({ page }) => {
    await page.goto('/automation')
    await page.waitForURL('**/automation/policies')
    await expect(page).toHaveURL(/\/automation\/policies$/)
  })

  test('/focus redirects to /focus/score', async ({ page }) => {
    await page.goto('/focus')
    await page.waitForURL('**/focus/score')
    await expect(page).toHaveURL(/\/focus\/score$/)
  })

  test('/reports redirects to /reports/activity', async ({ page }) => {
    await page.goto('/reports')
    await page.waitForURL('**/reports/activity')
    await expect(page).toHaveURL(/\/reports\/activity$/)
  })

  test('/privacy redirects to /privacy/data', async ({ page }) => {
    await page.goto('/privacy')
    await page.waitForURL('**/privacy/data')
    await expect(page).toHaveURL(/\/privacy\/data$/)
  })

  test('/updates redirects to /updates/status', async ({ page }) => {
    await page.goto('/updates')
    await page.waitForURL('**/updates/status')
    await expect(page).toHaveURL(/\/updates\/status$/)
  })

  test('/timeline redirects to /timeline/all', async ({ page }) => {
    await page.goto('/timeline')
    await page.waitForURL('**/timeline/all')
    await expect(page).toHaveURL(/\/timeline\/all$/)
  })

  test('/replay redirects to /replay/timeline', async ({ page }) => {
    await page.goto('/replay')
    await page.waitForURL('**/replay/timeline')
    await expect(page).toHaveURL(/\/replay\/timeline$/)
  })

  test('/coaching redirects to /coaching/goals', async ({ page }) => {
    await page.goto('/coaching')
    await page.waitForURL('**/coaching/goals')
    await expect(page).toHaveURL(/\/coaching\/goals$/)
  })

  test('/recalibration redirects to /recalibration/segments', async ({ page }) => {
    await page.goto('/recalibration')
    await page.waitForURL('**/recalibration/segments')
    await expect(page).toHaveURL(/\/recalibration\/segments$/)
  })

  test('/audit redirects to /audit/summary', async ({ page }) => {
    await page.goto('/audit')
    await page.waitForURL('**/audit/summary')
    await expect(page).toHaveURL(/\/audit\/summary$/)
  })
})
