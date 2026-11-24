import { describe, expect, it } from 'vitest'
import en from '../../i18n/locales/en.json'
import es from '../../i18n/locales/es.json'
import ja from '../../i18n/locales/ja.json'
import ko from '../../i18n/locales/ko.json'
import zhCN from '../../i18n/locales/zh-CN.json'
import { joinChildPath, navGroups, navSections, type RouteNode, routeTree } from '../route-tree'

// #13736: "Timeline" was both a front destination and Replay's first tab,
// "Coaching" both a page and a Settings tab, "Advanced" both a Settings group
// and a tab inside it. Two different places with one name make people open the
// wrong one, so every name the navigation shows — destinations, their tabs,
// bundle rows, the rail group and Settings' groups — must be unique per locale.

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const

interface NamedEntry {
  /** What the name points at: a path, or a bundle / group row. */
  id: string
  labelKey: string
}

function navEntries(tree: readonly RouteNode[]): NamedEntry[] {
  const entries: NamedEntry[] = []
  for (const route of tree) {
    // Routes without an icon are not listed anywhere (CommandPalette skips them).
    if (!route.icon) continue
    entries.push({ id: route.path, labelKey: route.labelKey })
    for (const child of route.children ?? []) {
      entries.push({ id: joinChildPath(route, child), labelKey: child.labelKey })
    }
    route.childGroups?.forEach((group, index) => {
      entries.push({ id: `${route.path}#group-${index}`, labelKey: group.labelKey })
    })
  }
  for (const section of navSections) entries.push({ id: `section:${section.id}`, labelKey: section.labelKey })
  for (const group of navGroups) entries.push({ id: `group:${group.id}`, labelKey: group.labelKey })
  return entries
}

function lookup(locale: unknown, key: string): string {
  const value = key.split('.').reduce<unknown>((node, part) => {
    if (node && typeof node === 'object') return (node as Record<string, unknown>)[part]
    return undefined
  }, locale)
  if (typeof value !== 'string') throw new Error(`missing label ${key}`)
  return value
}

function normalize(name: string): string {
  return name.normalize('NFKC').trim().replace(/\s+/g, ' ').toLocaleLowerCase()
}

/** Names shared by more than one entry, with the entries that share each. */
function duplicateNames(entries: readonly NamedEntry[], label: (key: string) => string): Record<string, string[]> {
  const byName = new Map<string, string[]>()
  for (const entry of entries) {
    const name = normalize(label(entry.labelKey))
    byName.set(name, [...(byName.get(name) ?? []), entry.id])
  }
  return Object.fromEntries([...byName].filter(([, ids]) => ids.length > 1))
}

describe('navigation names are unique (#13736)', () => {
  const entries = navEntries(routeTree)

  for (const [locale, messages] of Object.entries(LOCALES)) {
    it(`gives every destination its own name in ${locale}`, () => {
      expect(duplicateNames(entries, (key) => lookup(messages, key))).toEqual({})
    })
  }

  // Positive control: the check above must turn red when a name is reused.
  // Plant a route whose label repeats an existing destination's label.
  it('reports a planted duplicate in every locale', () => {
    const planted: NamedEntry[] = [...entries, { id: '/planted', labelKey: 'nav.timeline' }]
    for (const messages of Object.values(LOCALES)) {
      const duplicates = duplicateNames(planted, (key) => lookup(messages, key))
      expect(Object.values(duplicates)).toContainEqual(['/timeline', '/planted'])
    }
  })

  it('treats names that differ only in case or spacing as the same', () => {
    const names: Record<string, string> = { a: 'Data Export', b: ' data  export' }
    const duplicates = duplicateNames(
      [
        { id: '/a', labelKey: 'a' },
        { id: '/b', labelKey: 'b' },
      ],
      (key) => names[key],
    )
    expect(duplicates).toEqual({ 'data export': ['/a', '/b'] })
  })
})
