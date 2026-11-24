/**
 * #13738 — sentences that send the user somewhere name it as the screen shows
 * it.
 *
 * `reauth.noMethodBody` pointed at "Privacy settings" and
 * `onboarding.ai.consent.patternLearningDesc` at "Settings → Coaching", neither
 * of which is a menu name. The copy now reads "Settings → <tab label>" built
 * from the same `nav.settings` and `settings.tabs.*` keys the menu renders, so
 * renaming a tab or moving the setting fails here until the sentence follows.
 *
 * Korean copy is also held to the labels it quotes: two sentences gave the
 * settings path in English, and #13738 translated option and section labels
 * that other sentences quote.
 */

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { joinChildPath, routeTree } from '../../routes/route-tree'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const
type Locale = keyof typeof LOCALES
const ALL_LOCALES = Object.keys(LOCALES) as Locale[]
const FRONTEND_SRC = join(process.cwd(), 'src')

function copyAt(locale: Locale, path: string): string {
  const value = path
    .split('.')
    .reduce<unknown>((node, part) => (node as Record<string, unknown> | undefined)?.[part], LOCALES[locale])
  if (typeof value !== 'string') throw new Error(`${locale}: ${path} is not a string`)
  return value
}

function strings(node: unknown): string[] {
  if (typeof node === 'string') return [node]
  if (node !== null && typeof node === 'object') return Object.values(node).flatMap(strings)
  return []
}

/** The menu labels a user follows to reach `route`, outermost first. */
function menuLabelKeys(route: string): string[] {
  for (const node of routeTree) {
    if (node.path === route) return [node.labelKey]
    const leaf = node.children?.find((child) => joinChildPath(node, child) === route)
    if (leaf) return [node.labelKey, leaf.labelKey]
  }
  throw new Error(`no menu entry routes to ${route}`)
}

function source(relativePath: string): string {
  return readFileSync(join(FRONTEND_SRC, relativePath), 'utf8')
}

interface LocationCopy {
  readonly copyKey: string
  /** Where the setting the sentence talks about lives. */
  readonly route: string
  /** Source evidence that the setting is on that tab. */
  readonly evidence: { readonly file: string; readonly marker: string }
  /** Locales held to the menu path. All five unless the entry names fewer. */
  readonly locales?: readonly Locale[]
}

const LOCATION_COPY: readonly LocationCopy[] = [
  {
    // The re-authentication gate's "Open settings" button goes here, and the
    // PIN form (CaptureReauthSettings) is part of this tab.
    copyKey: 'reauth.noMethodBody',
    route: '/settings/privacy',
    evidence: { file: 'pages/setting-tabs/PrivacyTab.tsx', marker: '<CaptureReauthSettings' },
  },
  {
    // Coaching alerts are the `coaching.enabled` switch on the coaching tab.
    copyKey: 'onboarding.ai.consent.patternLearningDesc',
    route: '/settings/coaching',
    evidence: { file: 'pages/setting-tabs/CoachingSettingsTab.tsx', marker: "handleCoachingChange('enabled'" },
  },
  // Korean only below: the ja, zh-CN and es copy still names these menus in
  // English, and the English "Settings > Automation" predates the tab's
  // "AI & Automation" name. Both are outside this Korean-copy change.
  {
    // The hint's button opens this tab, whose AI features switch turns semantic search on.
    copyKey: 'search.semanticEnableHint',
    route: '/settings/advanced',
    evidence: { file: 'pages/setting-tabs/AdvancedTab.tsx', marker: "t('advancedTab.aiFeaturesMaster')" },
    locales: ['ko'],
  },
  {
    // The scene action switch (SceneIntelligenceConfig) is part of this tab.
    copyKey: 'replay.sceneExecutionDisabled',
    route: '/settings/ai-automation',
    evidence: {
      file: 'pages/setting-tabs/ai-automation/SceneIntelligenceConfig.tsx',
      marker: "onSceneIntelligenceChange('allow_action_execution'",
    },
    locales: ['ko'],
  },
]

interface LabelReference {
  readonly copyKey: string
  readonly labelKeys: readonly string[]
  /** The labels form a menu path, written outermost first with " → ". */
  readonly path?: boolean
}

/** Korean sentences that quote a label #13738 translated. */
const KO_LABEL_REFERENCES: readonly LabelReference[] = [
  // The Network & Server section is on the Advanced tab (AdvancedTab.tsx).
  {
    copyKey: 'settings.account.login.description',
    labelKeys: ['settings.tabs.advanced', 'settings.advanced.network'],
    path: true,
  },
  // The update channel picker (GeneralTab.tsx) shows these three options.
  {
    copyKey: 'settings.updateChannelUnavailable',
    labelKeys: ['settings.channelNightlyUnavailable', 'settings.channelStable', 'settings.channelPreRelease'],
  },
]

/** A label as a sentence quotes it: without a trailing note such as "(RC)". */
function quotedLabel(locale: Locale, labelKey: string): string {
  return copyAt(locale, labelKey).replace(/\s*\([^)]*\)$/, '')
}

const ENGLISH_SETTINGS_PATH = /\bSettings\s*[→>›]/

describe('settings location copy (#13738)', () => {
  it('the re-authentication gate opens the tab the copy names', () => {
    expect(source('components/CaptureReauthGate.tsx')).toContain("onGoToSettings={() => navigate('/settings/privacy')}")
  })

  it.each(LOCATION_COPY)('$copyKey: the setting is on $route', ({ evidence }) => {
    expect(source(evidence.file)).toContain(evidence.marker)
  })

  it.each(LOCATION_COPY)('$copyKey names the menu path', ({ copyKey, route, locales = ALL_LOCALES }) => {
    const labelKeys = menuLabelKeys(route)
    expect(labelKeys).toHaveLength(2)
    for (const locale of locales) {
      const location = labelKeys.map((labelKey) => copyAt(locale, labelKey)).join(' → ')
      expect(copyAt(locale, copyKey), `${locale}:${copyKey}`).toContain(location)
    }
  })

  it.each(KO_LABEL_REFERENCES)('ko $copyKey quotes the labels as shown', ({ copyKey, labelKeys, path }) => {
    const labels = labelKeys.map((labelKey) => quotedLabel('ko', labelKey))
    for (const label of labels) expect(label.trim().length).toBeGreaterThan(0)
    for (const text of path ? [labels.join(' → ')] : labels) {
      expect(copyAt('ko', copyKey)).toContain(text)
    }
  })

  it('Korean copy never gives a settings path in English', () => {
    // Positive control: the English copy does, so the pattern matches real paths.
    expect(strings(LOCALES.en).filter((value) => ENGLISH_SETTINGS_PATH.test(value)).length).toBeGreaterThan(0)
    expect(strings(LOCALES.ko).filter((value) => ENGLISH_SETTINGS_PATH.test(value))).toEqual([])
  })
})
