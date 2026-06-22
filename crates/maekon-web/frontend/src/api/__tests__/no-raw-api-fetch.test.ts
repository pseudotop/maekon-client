import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { describe, expect, it } from 'vitest'

const SRC_ROOT = join(process.cwd(), 'src')
const SOURCE_EXTENSIONS = new Set(['.ts', '.tsx'])
const RAW_API_FETCH_PATTERN = /fetch\s*\(\s*(['"`])\/api\b/g

function collectSourceFiles(dir: string, files: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    if (entry === 'node_modules' || entry === 'dist') continue
    const path = join(dir, entry)
    const stat = statSync(path)
    if (stat.isDirectory()) {
      collectSourceFiles(path, files)
      continue
    }
    const ext = path.endsWith('.tsx') ? '.tsx' : path.endsWith('.ts') ? '.ts' : ''
    if (SOURCE_EXTENSIONS.has(ext)) {
      files.push(path)
    }
  }
  return files
}

describe('API transport chokepoints', () => {
  it('does not call fetch with raw /api literals outside URL/auth helpers', () => {
    const offenders = collectSourceFiles(SRC_ROOT).flatMap((path) => {
      const content = readFileSync(path, 'utf8')
      const matches = [...content.matchAll(RAW_API_FETCH_PATTERN)].map((match) => match.index ?? 0)
      return matches.map((index) => {
        const sourcePath = join('src', relative(SRC_ROOT, path)).replace(/\\/g, '/')
        return `${sourcePath}:${content.slice(0, index).split('\n').length}`
      })
    })

    expect(offenders).toEqual([])
  })
})
