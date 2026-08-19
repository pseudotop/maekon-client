import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fetchApiJson, invokeIpc, navigateMain } from './helpers.js'

type PiiLevel = 'Off' | 'Basic' | 'Standard' | 'Strict'
type Phase = 'write' | 'restart'

type SettingsSnapshot = {
  privacy: { pii_filter_level: string }
}

type Evidence = {
  schema_version: 'maekon.windows-pii-persistence-evidence.v1'
  original_level: PiiLevel
  write?: {
    observed_at_utc: string
    initial_ui: PiiLevel
    before_save_ui: 'Strict'
    after_save_ui: 'Strict'
    after_save_api: 'Strict'
    on_disk: 'Strict'
  }
  restart?: {
    observed_at_utc: string
    post_restart_ui: 'Strict'
    post_restart_api: 'Strict'
    on_disk: 'Strict'
    restored_to: PiiLevel
  }
}

const CANONICAL_LEVELS = new Set<PiiLevel>(['Off', 'Basic', 'Standard', 'Strict'])
const phase = process.env.MAEKON_PII_PERSISTENCE_PHASE as Phase | undefined
const evidencePath = process.env.MAEKON_PII_EVIDENCE_PATH
const profileRoot = process.env.MAEKON_E2E_PROFILE_ROOT
const describePhase = phase && evidencePath && profileRoot ? describe : describe.skip

function requireLevel(value: string, field: string): PiiLevel {
  if (!CANONICAL_LEVELS.has(value as PiiLevel)) {
    throw new Error(`${field} must be a canonical PII level, got ${JSON.stringify(value)}`)
  }
  return value as PiiLevel
}

function configPath(): string {
  if (!profileRoot) throw new Error('MAEKON_E2E_PROFILE_ROOT is required')
  return join(profileRoot, 'roaming', 'maekon-e2e', 'config.json')
}

function readOnDiskLevel(): PiiLevel {
  const path = configPath()
  if (!existsSync(path)) throw new Error(`isolated config does not exist: ${path}`)
  const parsed = JSON.parse(readFileSync(path, 'utf8')) as SettingsSnapshot
  return requireLevel(parsed.privacy.pii_filter_level, 'config.privacy.pii_filter_level')
}

function readEvidence(): Evidence {
  if (!evidencePath) throw new Error('MAEKON_PII_EVIDENCE_PATH is required')
  return JSON.parse(readFileSync(evidencePath, 'utf8')) as Evidence
}

function writeEvidence(evidence: Evidence): void {
  if (!evidencePath) throw new Error('MAEKON_PII_EVIDENCE_PATH is required')
  mkdirSync(dirname(evidencePath), { recursive: true })
  writeFileSync(evidencePath, `${JSON.stringify(evidence, null, 2)}\n`, 'utf8')
}

async function openPrivacySettings() {
  await navigateMain('/settings/privacy')
  const select = await $('#privacy-pii-level')
  await select.waitForDisplayed({ timeout: 15_000 })
  return select
}

describePhase('Windows PII persistence process-boundary evidence (#9146)', () => {
  it('records the selected phase without exposing unrelated profile data', async () => {
    if (phase === 'write') {
      const before = await fetchApiJson<SettingsSnapshot>('/settings')
      const originalLevel = requireLevel(before.privacy.pii_filter_level, 'before API privacy.pii_filter_level')
      const select = await openPrivacySettings()
      const initialUi = requireLevel(await select.getValue(), 'initial UI privacy.pii_filter_level')

      await browser.execute((rawElement: HTMLElement) => {
        const element = rawElement as HTMLSelectElement
        const valueSetter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')?.set
        valueSetter?.call(element, 'Strict')
        element.dispatchEvent(new Event('input', { bubbles: true }))
        element.dispatchEvent(new Event('change', { bubbles: true }))
      }, select)
      await browser.waitUntil(() => select.getValue().then((value) => value === 'Strict'), {
        timeout: 5_000,
        timeoutMsg: 'PII level control did not accept Strict',
      })

      const save = await $('[data-testid="settings-save"]')
      await save.waitForClickable({ timeout: 10_000 })
      await save.click()

      await browser.waitUntil(
        async () => {
          const snapshot = await fetchApiJson<SettingsSnapshot>('/settings').catch(() => null)
          return (await select.getValue()) === 'Strict' && snapshot?.privacy.pii_filter_level === 'Strict'
        },
        { timeout: 15_000, timeoutMsg: 'saved UI and API values did not converge on Strict' },
      )
      await browser.waitUntil(() => existsSync(configPath()) && readOnDiskLevel() === 'Strict', {
        timeout: 10_000,
        timeoutMsg: 'isolated config.json did not persist Strict',
      })

      writeEvidence({
        schema_version: 'maekon.windows-pii-persistence-evidence.v1',
        original_level: originalLevel,
        write: {
          observed_at_utc: new Date().toISOString(),
          initial_ui: initialUi,
          before_save_ui: 'Strict',
          after_save_ui: 'Strict',
          after_save_api: 'Strict',
          on_disk: 'Strict',
        },
      })
      return
    }

    if (phase !== 'restart') throw new Error(`unsupported evidence phase: ${String(phase)}`)
    const evidence = readEvidence()
    if (!evidence.write) throw new Error('write-phase evidence is required before restart verification')

    const select = await openPrivacySettings()
    const postRestartUi = requireLevel(await select.getValue(), 'post-restart UI privacy.pii_filter_level')
    const afterRestart = await fetchApiJson<SettingsSnapshot>('/settings')
    const postRestartApi = requireLevel(
      afterRestart.privacy.pii_filter_level,
      'post-restart API privacy.pii_filter_level',
    )
    const onDisk = readOnDiskLevel()

    expect(postRestartUi).toBe('Strict')
    expect(postRestartApi).toBe('Strict')
    expect(onDisk).toBe('Strict')

    if (evidence.original_level !== 'Strict') {
      await invokeIpc('update_setting', {
        configJson: JSON.stringify({ privacy: { pii_filter_level: evidence.original_level } }),
      })
      await browser.waitUntil(() => readOnDiskLevel() === evidence.original_level, {
        timeout: 10_000,
        timeoutMsg: `isolated config.json was not restored to ${evidence.original_level}`,
      })
    }

    writeEvidence({
      ...evidence,
      restart: {
        observed_at_utc: new Date().toISOString(),
        post_restart_ui: 'Strict',
        post_restart_api: 'Strict',
        on_disk: 'Strict',
        restored_to: evidence.original_level,
      },
    })
  })
})
