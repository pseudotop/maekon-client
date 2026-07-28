// #8056 P2-2: Settings → Sync setup client. Enabling cross-device sync and
// storing its passphrase go through the local REST API (there is no Tauri
// command for these), so the passphrase lands in the OS keychain and a
// Finder/Start-menu launch can activate sync without an env var.
import { resolveApiUrl, withResolvedLocalAuthHeaders } from '../utils/api-base'

export interface SyncSetupStatus {
  enabled: boolean
  transport: string
  passphrase_set: boolean
  restart_required: boolean
}

export interface SyncSetupRequest {
  // Omit to leave the stored passphrase untouched (pure enable/disable).
  passphrase?: string
  enabled: boolean
  transport?: 'file' | 'lan' | 'remote'
}

export async function fetchSyncSetup(): Promise<SyncSetupStatus> {
  const url = await resolveApiUrl('/api/sync/setup')
  const response = await fetch(url, await withResolvedLocalAuthHeaders())
  if (!response.ok) {
    throw new Error(`Failed to fetch sync setup: ${response.statusText}`)
  }
  return response.json()
}

export async function updateSyncSetup(request: SyncSetupRequest): Promise<SyncSetupStatus> {
  const url = await resolveApiUrl('/api/sync/setup')
  const init = await withResolvedLocalAuthHeaders({
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(request),
  })
  const response = await fetch(url, init)
  if (!response.ok) {
    // The handler returns a plain-text reason for 4xx (e.g. passphrase too short).
    const detail = await response.text().catch(() => '')
    throw new Error(detail || `Failed to update sync setup: ${response.statusText}`)
  }
  return response.json()
}
