import { test as base, expect } from '@playwright/test'
import { mockBackgroundStreams, mockDefaultApiFallbacks } from './mock-api'

const test = base.extend({
  page: async ({ page }, use) => {
    await page.addInitScript(() => {
      window.localStorage.setItem('oneshim-web-standalone-mode', '0')
    })
    await mockDefaultApiFallbacks(page)
    await mockBackgroundStreams(page)
    await use(page)
  },
})

export { test, expect }
export type { Page, Request, Route } from '@playwright/test'
