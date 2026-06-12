import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

describe('asset entrypoints', () => {
  it('points the app favicon at the Maekon brand asset instead of the Vite placeholder', () => {
    const indexHtml = readFileSync(join(process.cwd(), 'index.html'), 'utf-8')

    expect(indexHtml).not.toContain('/vite.svg')
    expect(indexHtml).toContain('href="/favicon.svg"')
  })
})
