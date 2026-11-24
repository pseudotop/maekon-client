import { describe, expect, it } from 'vitest'
import { aiCapabilityReadiness, aiReadinessActionRoute } from '../../features/aiReadiness'
import { joinChildPath, routeTree } from '../../routes/route-tree'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const

function copyAt(locale: keyof typeof LOCALES, path: string): unknown {
  return path
    .split('.')
    .reduce<unknown>((node, part) => (node as Record<string, unknown> | undefined)?.[part], LOCALES[locale])
}

/** Keys #13737 added for the shared display path and its fallback sentences. */
const ERROR_DISPLAY_KEYS = [
  'errors.generic',
  'errors.tryAgainLater',
  'errors.supportDetails',
  'errors.copySupportDetails',
  'errors.supportDetailsCopied',
  'aiReadiness.unknownReason',
  'chat.rename_failed',
  'chat.suggestions_failed',
  'chat.response_failed',
  'replay.timelineLoadFailed',
  'syncTab.syncFailed',
  'syncTab.setupFailed',
  'focus.metricsLoadFailed',
  'settings.updateActionFailed',
  'settings.permissionRequestFailed',
  'settings.permissionOpenSystemSettingsFailed',
  'settings.notifTestFailed',
  'settings.tagActionFailed',
  'settingsAutomation.modelDiscoveryFailed',
  'privacy.claims.retract.failed',
] as const

/**
 * Sentences that send the user to the consent switches. They must name the
 * menu the switches actually live in (#13737), which is where the backend's
 * `open_privacy_consent` action routes.
 */
const CONSENT_LOCATION_KEYS = [
  'chat.full_text_consent_required',
  'settingsAutomation.chatVerification.result.consentRequired',
  'advancedTab.tieredMemoryNeedsConsent',
] as const

/** The menu labels a user follows to reach `route`, outermost first. */
function menuLabelKeys(route: string): string[] {
  for (const node of routeTree) {
    if (node.path === route) return [node.labelKey]
    const leaf = node.children?.find((child) => joinChildPath(node, child) === route)
    if (leaf) return [node.labelKey, leaf.labelKey]
  }
  throw new Error(`no menu entry routes to ${route}`)
}

describe('error display copy parity (#13737)', () => {
  it.each(ERROR_DISPLAY_KEYS)('%s has copy in every locale', (key) => {
    for (const locale of Object.keys(LOCALES) as Array<keyof typeof LOCALES>) {
      const copy = copyAt(locale, key)
      expect(typeof copy, `${locale}:${key}`).toBe('string')
      expect((copy as string).trim(), `${locale}:${key}`).not.toBe('')
    }
  })

  it('keeps fallback sentences free of interpolation the display path never fills', () => {
    for (const locale of Object.keys(LOCALES) as Array<keyof typeof LOCALES>) {
      for (const key of ERROR_DISPLAY_KEYS) {
        expect(copyAt(locale, key) as string, `${locale}:${key}`).not.toMatch(/\{\{?\s*\w+\s*\}\}?/)
      }
    }
  })

  it('names the menu the consent switches live in', () => {
    const route = aiReadinessActionRoute({
      ...aiCapabilityReadiness(null, 'chat.subprocess'),
      action: 'open_privacy_consent',
    })
    expect(route).not.toBeNull()
    const labelKeys = menuLabelKeys(route as string)

    for (const locale of Object.keys(LOCALES) as Array<keyof typeof LOCALES>) {
      const location = labelKeys.map((labelKey) => copyAt(locale, labelKey) as string).join(' → ')
      for (const key of CONSENT_LOCATION_KEYS) {
        expect(copyAt(locale, key) as string, `${locale}:${key}`).toContain(location)
      }
    }
  })
})
