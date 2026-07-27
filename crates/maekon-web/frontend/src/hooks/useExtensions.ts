import { useCallback, useEffect, useState } from 'react'

/**
 * Extension registry hook (ADR-029, #8586).
 *
 * Exposes the eight orthogonal readiness axes exactly as the backend reports
 * them. `summary_label` is intentionally nullable: when the backend cannot name
 * an honest single state, the UI must render the axes instead of inventing
 * "connected".
 *
 * Every mutation echoes the row's `revision` so a stale double-click loses the
 * compare-and-swap instead of applying twice.
 */

export type InstallationState =
  | 'not_installed'
  | 'installing'
  | 'installed'
  | 'install_failed'
  | 'uninstalling'
  | 'uninstalled'

export type Enablement = 'disabled' | 'enabled'

export type AccountAuthentication =
  | 'not_required'
  | 'unauthenticated'
  | 'authenticating'
  | 'authenticated'
  | 'revoked'
  | 'auth_error'

export type CapabilityGrant = 'not_requested' | 'pending' | 'granted' | 'denied' | 'revoked' | 'expired'

export type UpdateState =
  | 'current'
  | 'update_available'
  | 'staged'
  | 'activating'
  | 'update_failed'
  | 'rollback_available'

export interface Availability {
  state: 'available' | 'unavailable'
  reason?: string
}

export interface Health {
  state: 'unknown' | 'healthy' | 'degraded' | 'unhealthy'
  reason?: string
}

export interface ExtensionView {
  install_id: string
  extension_id: string
  version: string
  provenance: 'BUNDLED' | 'CURATED_LOCAL'
  availability: Availability
  installation: InstallationState
  enablement: Enablement
  authentication: AccountAuthentication
  grant: CapabilityGrant
  update: UpdateState
  health: Health
  previous_version: string | null
  revision: number
  summary_label: string | null
}

export interface ExtensionOutcome {
  outcome: string
  [key: string]: unknown
}

/** Lazy-loaded invoke — graceful degradation outside Tauri (ADR-004). */
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: inv } = await import('@tauri-apps/api/core')
  return inv<T>(cmd, args)
}

export interface UseExtensions {
  extensions: ExtensionView[]
  loading: boolean
  error: string | null
  refresh: () => Promise<void>
  install: (installId: string, revision: number) => Promise<ExtensionOutcome>
  setEnablement: (installId: string, enabled: boolean, revision: number) => Promise<ExtensionOutcome>
  rollback: (installId: string, revision: number) => Promise<ExtensionOutcome>
  uninstall: (installId: string, revision: number, completedCleanup: string[]) => Promise<ExtensionOutcome>
}

export function useExtensions(): UseExtensions {
  const [extensions, setExtensions] = useState<ExtensionView[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      setExtensions(await invoke<ExtensionView[]>('list_extensions'))
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const install = useCallback(
    async (installId: string, revision: number) => {
      const result = await invoke<ExtensionOutcome>('install_extension', {
        installId,
        expectedRevision: revision,
      })
      await refresh()
      return result
    },
    [refresh],
  )

  const setEnablement = useCallback(
    async (installId: string, enabled: boolean, revision: number) => {
      const result = await invoke<ExtensionOutcome>('set_extension_enablement', {
        installId,
        enabled,
        expectedRevision: revision,
      })
      await refresh()
      return result
    },
    [refresh],
  )

  const rollback = useCallback(
    async (installId: string, revision: number) => {
      const result = await invoke<ExtensionOutcome>('rollback_extension', {
        installId,
        expectedRevision: revision,
      })
      await refresh()
      return result
    },
    [refresh],
  )

  const uninstall = useCallback(
    async (installId: string, revision: number, completedCleanup: string[]) => {
      const result = await invoke<ExtensionOutcome>('uninstall_extension', {
        installId,
        expectedRevision: revision,
        completedCleanup,
      })
      await refresh()
      return result
    },
    [refresh],
  )

  return { extensions, loading, error, refresh, install, setEnablement, rollback, uninstall }
}
