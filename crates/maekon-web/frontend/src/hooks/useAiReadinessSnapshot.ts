import { useEffect, useState } from 'react'
import { fetchFeatureCapabilities } from '../api/client'
import type { FeatureCapabilitySnapshot } from '../api/contracts'
import { isStandaloneModeEnabled } from '../api/standalone'

const CACHE_TTL_MS = 300_000
const RETRY_DELAY_MS = 30_000

interface CachedSnapshot {
  expiresAt: number
  snapshot: FeatureCapabilitySnapshot
}

interface InFlightSnapshot {
  generation: number
  promise: Promise<FeatureCapabilitySnapshot>
}

let cachedSnapshot: CachedSnapshot | null = null
let cacheGeneration = 0
let inFlightRequest: InFlightSnapshot | null = null
const refreshSubscribers = new Set<() => void>()

function canQueryDesktopCapabilities(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window && !isStandaloneModeEnabled()
}

function freshCachedSnapshot(): FeatureCapabilitySnapshot | undefined {
  if (!cachedSnapshot || cachedSnapshot.expiresAt <= Date.now()) return undefined
  return cachedSnapshot.snapshot
}

async function loadSnapshot(force = false): Promise<FeatureCapabilitySnapshot> {
  if (!force) {
    const cached = freshCachedSnapshot()
    if (cached) return cached
  }
  const generation = cacheGeneration
  if (inFlightRequest?.generation === generation) return inFlightRequest.promise

  let request: Promise<FeatureCapabilitySnapshot>
  request = fetchFeatureCapabilities()
    .then((snapshot) => {
      if (generation !== cacheGeneration) return loadSnapshot()
      cachedSnapshot = { snapshot, expiresAt: Date.now() + CACHE_TTL_MS }
      return snapshot
    })
    .finally(() => {
      if (inFlightRequest?.promise === request) inFlightRequest = null
    })
  inFlightRequest = { generation, promise: request }
  return request
}

/**
 * Invalidate the cross-surface cache after configuration or consent changes.
 * Mounted consumers refresh from the backend-owned readiness evaluator.
 */
export function invalidateAiReadinessSnapshotCache(): void {
  cacheGeneration += 1
  cachedSnapshot = null
  for (const refresh of refreshSubscribers) refresh()
}

/**
 * Shared bounded cache for Settings, Chat, suggestions, and summary surfaces.
 * The backend command owns provider probing and the authoritative readiness
 * result; consumers never issue an LLM request or recompute readiness. This
 * hook deliberately has no QueryClient dependency because the overlay has a
 * separate React root.
 */
export function useAiReadinessSnapshot(): FeatureCapabilitySnapshot | undefined {
  const [snapshot, setSnapshot] = useState<FeatureCapabilitySnapshot | undefined>(() => freshCachedSnapshot())

  useEffect(() => {
    if (!canQueryDesktopCapabilities()) return

    let active = true
    let expiryTimer: ReturnType<typeof setTimeout> | undefined
    const scheduleExpiryRefresh = () => {
      if (expiryTimer) clearTimeout(expiryTimer)
      if (!cachedSnapshot) return
      const delay = Math.max(0, cachedSnapshot.expiresAt - Date.now())
      expiryTimer = setTimeout(() => refresh(true), delay)
    }
    const refresh = (force = false) => {
      if (expiryTimer) clearTimeout(expiryTimer)
      if (force) setSnapshot(undefined)
      void loadSnapshot(force)
        .then((nextSnapshot) => {
          if (!active) return
          setSnapshot(nextSnapshot)
          scheduleExpiryRefresh()
        })
        .catch(() => {
          // The product surfaces remain fail-closed until a typed backend
          // snapshot is available. Retry one bounded timer at a time so a
          // transient IPC failure does not strand mounted surfaces forever.
          if (!active) return
          setSnapshot(undefined)
          expiryTimer = setTimeout(() => refresh(true), RETRY_DELAY_MS)
        })
    }
    const handleInvalidated = () => refresh(true)

    refresh()
    refreshSubscribers.add(handleInvalidated)
    return () => {
      active = false
      if (expiryTimer) clearTimeout(expiryTimer)
      refreshSubscribers.delete(handleInvalidated)
    }
  }, [])

  return snapshot
}
