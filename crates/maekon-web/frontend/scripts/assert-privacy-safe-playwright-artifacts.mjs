#!/usr/bin/env node

import { readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'

const root = process.argv[2] || 'test-results/privacy-safe-web-verification'
const forbiddenExtensions = new Set(['.jpeg', '.jpg', '.png', '.webm', '.zip'])
const forbiddenNames = new Set(['trace.zip'])
const violations = []

function walk(path) {
  let entries
  try {
    entries = readdirSync(path, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') {
      return
    }
    throw error
  }

  for (const entry of entries) {
    const child = join(path, entry.name)
    if (entry.isDirectory()) {
      walk(child)
      continue
    }
    if (!entry.isFile()) {
      continue
    }
    const lowerName = entry.name.toLowerCase()
    const extension = lowerName.includes('.') ? lowerName.slice(lowerName.lastIndexOf('.')) : ''
    if (forbiddenNames.has(lowerName) || forbiddenExtensions.has(extension)) {
      violations.push(child)
      continue
    }
    if (statSync(child).size > 1024 * 1024) {
      violations.push(`${child} (too large for privacy-safe evidence)`)
    }
  }
}

walk(root)

if (violations.length > 0) {
  console.error(`privacy-safe Playwright artifact guard failed:\n${violations.join('\n')}`)
  process.exit(1)
}
