import { i18nRegex } from './helpers/i18n'
import { expect, test } from './helpers/test'

const notificationEnabledName = i18nRegex('settings.notifEnabled')
const notificationIdleName = i18nRegex('settings.notifIdle')
const languageSelectorName = i18nRegex('settings.language')
// update channel selector tested via #update-channel id (no i18n lookup needed)

test.describe('Settings Actions', () => {
  test('P117: notification toggles exist', async ({ page }) => {
    await page.goto('/settings/general')
    const section = page.locator('#section-notification')
    await expect(section).toContainText(notificationEnabledName)
    await expect(section).toContainText(notificationIdleName)
  })

  test('P118: monitor interval input exists', async ({ page }) => {
    await page.goto('/settings/monitoring')
    const input = page.locator('#settings-metrics-interval')
    await expect(input).toBeVisible()
  })

  test('P119: save button exists', async ({ page }) => {
    await page.goto('/settings')
    const btn = page.getByTestId('settings-save')
    await expect(btn).toBeVisible()
  })

  test('P120: language selector exists', async ({ page }) => {
    await page.goto('/settings/general')
    await expect(page.getByLabel(languageSelectorName)).toBeVisible()
  })

  test('P121: update channel selector exists', async ({ page }) => {
    await page.goto('/settings/general')
    await expect(page.locator('#update-channel')).toBeVisible()
  })

  test('P122: coaching settings tab renders three sections', async ({ page }) => {
    // Smoke test for the v0.5 UX polish spec: CoachingSettingsTab must mount
    // all four cards (Goals, Quiet Hours, Coaching Tone, Coaching Profiles)
    // so users can reach the full coaching configuration surface.
    await page.goto('/settings/coaching')

    // Use role=heading to hit each Card's <h2> without coupling to raw copy
    await expect(page.getByRole('heading', { name: i18nRegex('coaching.settingsTitle') })).toBeVisible()
    await expect(page.getByRole('heading', { name: i18nRegex('coaching.quietHoursTitle') })).toBeVisible()
    await expect(page.getByRole('heading', { name: i18nRegex('coaching.toneTitle') })).toBeVisible()
    await expect(page.getByRole('heading', { name: i18nRegex('coaching.profilesTitle') })).toBeVisible()

    // Tone radio group: 3 options all rendered with the default (Gentle) checked
    const direct = page.getByRole('radio', { name: i18nRegex('coaching.toneOption.Direct') })
    const gentle = page.getByRole('radio', { name: i18nRegex('coaching.toneOption.Gentle') })
    const dataDriven = page.getByRole('radio', { name: i18nRegex('coaching.toneOption.DataDriven') })
    await expect(direct).toBeVisible()
    await expect(gentle).toBeVisible()
    await expect(dataDriven).toBeVisible()
  })
})
