import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')

function fail(message) {
  console.error(message)
  process.exit(1)
}

const packageJson = JSON.parse(readFileSync(resolve(rootDir, 'package.json'), 'utf8'))
const workspaceConfig = readFileSync(resolve(rootDir, 'pnpm-workspace.yaml'), 'utf8')

function readTopLevelMap(source, key) {
  const result = {}
  const lines = source.split(/\r?\n/)
  const headerIndex = lines.findIndex((line) => line === `${key}:`)
  if (headerIndex < 0) return result

  for (const line of lines.slice(headerIndex + 1)) {
    if (!line.trim() || line.trimStart().startsWith('#')) continue
    if (!line.startsWith('  ')) break

    const match = line.match(/^  (?:(['"])(.*?)\1|([^:]+)):\s*(.*?)\s*$/)
    if (!match) continue
    const entryKey = (match[2] || match[3]).trim()
    const rawValue = match[4]
    result[entryKey] = rawValue === 'true' ? true : rawValue === 'false' ? false : rawValue
  }

  return result
}

if (packageJson.pnpm && Object.keys(packageJson.pnpm).length > 0) {
  fail('pnpm settings must live in pnpm-workspace.yaml for pnpm 11.')
}

// CLI 출력 형식은 pnpm 10 부버전마다 달라지므로 workspace SSOT를 직접 검증한다.
const overrides = readTopLevelMap(workspaceConfig, 'overrides')
if (Object.keys(overrides).length === 0) {
  fail('pnpm overrides are not configured in pnpm-workspace.yaml.')
}

const expectedOverrides = {
  'serialize-javascript': '7.0.5',
  '@babel/core': '7.29.6',
  'form-data': '4.0.6',
  'js-yaml': '4.3.0',
  ws: '8.21.0',
  'cheerio>undici': '7.28.0',
  'minimatch@10.2.5>brace-expansion': '5.0.7',
  'webdriver>undici': '6.27.0',
}

for (const [packageName, expectedVersion] of Object.entries(expectedOverrides)) {
  if (overrides[packageName] !== expectedVersion) {
    fail(`${packageName} must be overridden to ${expectedVersion}.`)
  }
}

const allowBuilds = readTopLevelMap(workspaceConfig, 'allowBuilds')
if (Object.keys(allowBuilds).length === 0) {
  fail('pnpm allowBuilds are not configured in pnpm-workspace.yaml.')
}

const expectedBuildApprovals = {
  edgedriver: false,
  esbuild: true,
  geckodriver: false,
}

for (const [packageName, expectedApproval] of Object.entries(expectedBuildApprovals)) {
  if (allowBuilds[packageName] !== expectedApproval) {
    fail(`${packageName} must be set to ${expectedApproval} in allowBuilds.`)
  }
}
