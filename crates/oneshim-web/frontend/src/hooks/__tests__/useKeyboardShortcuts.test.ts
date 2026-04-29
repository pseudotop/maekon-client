import { describe, expect, it } from 'vitest'
import { getShortcutsList } from '../useKeyboardShortcuts'

describe('getShortcutsList', () => {
  it('returns 11 shortcut entries', () => {
    const list = getShortcutsList()
    expect(list).toHaveLength(11)
  })

  it('each entry has key and descriptionKey', () => {
    const list = getShortcutsList()
    list.forEach((entry) => {
      expect(entry).toHaveProperty('key')
      expect(entry).toHaveProperty('descriptionKey')
      expect(typeof entry.key).toBe('string')
      expect(typeof entry.descriptionKey).toBe('string')
    })
  })

  it('includes D, T, S, P shortcuts', () => {
    const list = getShortcutsList()
    const keys = list.map((e) => e.key)
    expect(keys).toContain('D')
    expect(keys).toContain('T')
    expect(keys).toContain('S')
    expect(keys).toContain('P')
  })

  it('includes modifier key shortcuts', () => {
    const list = getShortcutsList()
    const keys = list.map((e) => e.key)
    // Should have Cmd/Ctrl+B and Cmd/Ctrl+K
    const modShortcuts = keys.filter((k) => k.includes('B') || k.includes('K'))
    expect(modShortcuts.length).toBeGreaterThanOrEqual(2)
  })
})
