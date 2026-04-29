import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ExecutionPolicyConfig } from '../contracts'

const STANDALONE_STORAGE_KEY = 'oneshim-web-standalone-mode'

function policy(overrides: Partial<ExecutionPolicyConfig> = {}): ExecutionPolicyConfig {
  return {
    policy_id: 'pol-git-status',
    process_name: 'git',
    process_hash: null,
    allowed_args: ['status'],
    requires_sudo: false,
    max_execution_time_ms: 5000,
    audit_level: 'Basic',
    sandbox_profile: null,
    allowed_paths: [],
    allow_network: null,
    require_signed_token: false,
    confirmation: 'Confirm',
    ...overrides,
  }
}

describe('standalone execution policy API', () => {
  const originalUrl = window.location.href

  afterEach(() => {
    window.localStorage.clear()
    window.history.replaceState({}, '', originalUrl)
    vi.resetModules()
  })

  it('returns an array for execution policies instead of the generic ok fallback', async () => {
    window.localStorage.setItem(STANDALONE_STORAGE_KEY, '1')
    const { handleStandaloneRequest } = await import('../standalone')

    const response = await handleStandaloneRequest('/api/automation/execution-policies', undefined, true)
    const body = await response?.json()

    expect(response?.ok).toBe(true)
    expect(body).toEqual([])
  })

  it('stores created execution policies for standalone UI smoke checks', async () => {
    window.localStorage.setItem(STANDALONE_STORAGE_KEY, '1')
    const { handleStandaloneRequest } = await import('../standalone')

    await handleStandaloneRequest(
      '/api/automation/execution-policies',
      {
        method: 'POST',
        body: JSON.stringify(policy()),
      },
      true,
    )

    const response = await handleStandaloneRequest('/api/automation/execution-policies', undefined, true)
    const body = await response?.json()

    expect(body).toEqual([policy()])
  })
})
