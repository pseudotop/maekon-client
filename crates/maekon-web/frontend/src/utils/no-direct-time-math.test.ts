/**
 * #13740: pages and components leave duration arithmetic to `utils/formatters`.
 *
 * A screen that divides seconds by 60 or 3600 itself ends up with its own
 * format and its own hard-coded "h"/"m" units, which is how the same length of
 * time came to read "6h 7m" on one card and "6h 7m 0s" on the next. This guard
 * parses every page and component and rejects `/ 60`, `% 60`, `/ 3600` and
 * `% 3600` (numeric separators included). String contents such as Tailwind's
 * `bg-brand-signal/60` are not expressions, so they never match.
 */

import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import ts from 'typescript'
import { describe, expect, it } from 'vitest'

const FRONTEND_ROOT = process.cwd()
const SCANNED_DIRS = ['src/pages', 'src/components']
const TIME_DIVISORS = new Set([60, 3600])
const FLAGGED_OPERATORS = new Set<ts.SyntaxKind>([
  ts.SyntaxKind.SlashToken,
  ts.SyntaxKind.PercentToken,
  ts.SyntaxKind.SlashEqualsToken,
  ts.SyntaxKind.PercentEqualsToken,
])

/**
 * Direct calculations that sit in files another open issue is changing. Each
 * count is a ceiling: it may only go down. Delete the entry once that change
 * moves the calculation onto `utils/formatters`.
 */
const PENDING_ELSEWHERE: Readonly<Record<string, { readonly max: number; readonly owner: string }>> = {}

function unwrap(node: ts.Expression): ts.Expression {
  let current = node
  while (ts.isParenthesizedExpression(current)) current = current.expression
  return current
}

/** 1-based line numbers of every `x / 60`-style expression in `source`. */
function findTimeDivisions(source: string, fileName = 'inline.tsx'): number[] {
  const kind = fileName.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS
  const file = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true, kind)
  const lines: number[] = []
  const visit = (node: ts.Node) => {
    if (ts.isBinaryExpression(node) && FLAGGED_OPERATORS.has(node.operatorToken.kind)) {
      const divisor = unwrap(node.right)
      if (ts.isNumericLiteral(divisor) && TIME_DIVISORS.has(Number(divisor.text.replace(/_/g, '')))) {
        lines.push(file.getLineAndCharacterOfPosition(node.getStart(file)).line + 1)
      }
    }
    ts.forEachChild(node, visit)
  }
  visit(file)
  return lines
}

function sourceFiles(dir: string, files: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) {
      if (entry !== '__tests__') sourceFiles(path, files)
    } else if (/\.tsx?$/.test(entry) && !/\.(test|stories)\.tsx?$/.test(entry)) {
      files.push(path)
    }
  }
  return files
}

function scan(): Map<string, number[]> {
  const found = new Map<string, number[]>()
  for (const dir of SCANNED_DIRS) {
    for (const path of sourceFiles(join(FRONTEND_ROOT, dir))) {
      const lines = findTimeDivisions(readFileSync(path, 'utf8'), path)
      if (lines.length > 0) found.set(relative(FRONTEND_ROOT, path).replace(/\\/g, '/'), lines)
    }
  }
  return found
}

describe('no direct time arithmetic in pages and components (#13740)', () => {
  it('formats durations through utils/formatters instead of dividing by 60 or 3600', () => {
    const offenders: string[] = []
    for (const [file, lines] of scan()) {
      const allowed = PENDING_ELSEWHERE[file]?.max ?? 0
      if (lines.length > allowed) offenders.push(...lines.map((line) => `${file}:${line}`))
    }
    // Use formatDuration, formatMinutes, formatHours, formatClock, minutesFromSeconds, hoursFromSeconds
    // or largestElapsedUnit from utils/formatters, or add the missing helper there with a test.
    expect(offenders).toEqual([])
  })

  it('lists only files that still exist', () => {
    const missing = Object.keys(PENDING_ELSEWHERE).filter((file) => {
      try {
        return !statSync(join(FRONTEND_ROOT, file)).isFile()
      } catch {
        return true
      }
    })
    expect(missing).toEqual([])
  })

  it('catches the calculations it exists to stop', () => {
    expect(findTimeDivisions('const m = Math.round(secs / 60)')).toEqual([1])
    expect(findTimeDivisions('const h = Math.floor(secs / (3_600))')).toEqual([1])
    expect(findTimeDivisions('const m = Math.floor((secs % 3600) / 60)')).toEqual([1, 1])
    expect(findTimeDivisions('let m = secs\nm /= 60')).toEqual([2])
  })

  it('ignores look-alikes that are not time arithmetic', () => {
    expect(findTimeDivisions(`const c = 'bg-brand-signal/60'`)).toEqual([])
    expect(findTimeDivisions('const el = <div className="text-content-inverse/60" />')).toEqual([])
    expect(findTimeDivisions('const ms = minutes * 60')).toEqual([])
    expect(findTimeDivisions('const pct = used / 600')).toEqual([])
  })
})
