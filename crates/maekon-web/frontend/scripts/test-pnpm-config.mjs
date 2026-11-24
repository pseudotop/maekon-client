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

// #11396: these versions are SECURITY pins, so restating them here is what
// keeps a silent downgrade from landing. The same property made them stale —
// three of them had drifted into being the vulnerable version themselves while
// this list asserted they were correct. Re-check against open Dependabot
// alerts when touching this block; a pin that is never re-measured stops being
// a guard and becomes the thing holding the vulnerability in place.
const expectedOverrides = {
  // #13264: bound indexed source-map offsets without pinning safe releases.
  'source-map-js@>=1.0.0 <1.2.2': '1.2.2',
  // #13584: preserve parser 6.x and the serializer's vulnerable-range floor.
  'postcss-selector-parser@>=6.1.0 <6.1.5': '6.1.5',
  'serialize-javascript@<7.1.3': '7.1.3',
  '@babel/core': '7.29.6',
  'form-data': '4.0.6',
  // #12497: the 4.3.1 pin itself became vulnerable (GHSA-2883-xcg3-v3hh).
  'js-yaml@<4.3.2': '4.3.2',
  ws: '8.21.0',
  // #12763: fix IPv6 link-local and local-use NAT64 classification.
  // #12783: the 10.5.1 pin itself became vulnerable (GHSA-j6r3-76f7-8jcv).
  // #13584: 10.7.1 became vulnerable to GHSA-rxp7-6hxx-x8q5.
  'ip-address@<10.7.4': '10.7.4',
  // #12783: GHSA-q2hr-2g5m-vwhr across the three in-tree brace-expansion lines.
  'brace-expansion@<1.1.21': '1.1.21',
  'brace-expansion@>=2.0.0 <2.1.7': '2.1.7',
  'brace-expansion@>=4.0.0 <5.0.12': '5.0.12',
  // #13555: the cheerio>undici 7.29.1 and webdriver>undici 6.28.1 pins became
  // the vulnerable versions. Keep the patched floors, not those exact parents.
  'undici@>=6.0.0 <6.29.1': '6.29.1',
  'undici@>=7.0.0 <7.30.1': '7.30.1',
  'nanoid@<3.3.18': '3.3.18',
  'basic-ftp@<=6.2.0': '6.2.5',
  'deepmerge-ts@<8.0.0': '8.0.2',
}

for (const [packageName, expectedVersion] of Object.entries(expectedOverrides)) {
  if (overrides[packageName] !== expectedVersion) {
    fail(`${packageName} must be overridden to ${expectedVersion}.`)
  }
}

for (const packageName of [
  'source-map-js',
  'postcss-selector-parser',
  'serialize-javascript',
  'ip-address',
]) {
  if (Object.hasOwn(overrides, packageName)) {
    fail(`${packageName} must use a vulnerable-range override, not an unconditional pin.`)
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
