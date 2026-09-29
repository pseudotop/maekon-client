import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchProviderSurfaces, fetchSettings } from '../../../api/client'
import type { ProviderSurfaceCatalog, ProviderSurfaceSpec } from '../../../api/contracts'
import { DEFAULT_PROVIDER_SURFACE_CATALOG } from '../../../api/defaultProviderSurfaceCatalog'
import { providerSurfaceByIdFromList } from '../../../features/providerSurfaces'
import { addToast } from '../../../hooks/useToast'
import type { SessionInfo } from '../types'
import { errorMessage, filterHttpApiSurfaces, ipc } from '../utils'

export function useSessionSetup() {
  const { t } = useTranslation()
  const [providerCatalog, setProviderCatalog] = useState<ProviderSurfaceCatalog>(DEFAULT_PROVIDER_SURFACE_CATALOG)
  const [catalogLoaded, setCatalogLoaded] = useState(false)
  // #12559: the HTTP surface saved in Settings (`llm_api.surface_id`). #12541 sends the saved key
  // only to that surface, so starting on another one reads as "not configured".
  // undefined while Settings load, null when nothing is saved.
  const [configuredSurfaceId, setConfiguredSurfaceId] = useState<string | null | undefined>(undefined)
  const [httpSurfaceId, setHttpSurfaceId] = useState<string>('')
  const [sessions, setSessions] = useState<SessionInfo[]>([])
  const [tokenUsage, setTokenUsage] = useState<{ total: number; budget: number | null }>({ total: 0, budget: null })
  const [sessionLoadError, setSessionLoadError] = useState<string | null>(null)

  // Fetch provider catalog dynamically, fall back to static import
  useEffect(() => {
    fetchProviderSurfaces()
      .then(setProviderCatalog)
      .catch(() => {}) // keep static fallback
      .finally(() => setCatalogLoaded(true))
  }, [])

  useEffect(() => {
    fetchSettings()
      .then((settings) => setConfiguredSurfaceId(settings.ai_provider.llm_api?.surface_id ?? null))
      .catch(() => setConfiguredSurfaceId(null)) // fall back to the first surface
  }, [])

  const httpApiSurfaces: ProviderSurfaceSpec[] = useMemo(
    () => filterHttpApiSurfaces(providerCatalog),
    [providerCatalog],
  )

  // Pick the configured surface, or the first one when it is missing from the list. This sets the
  // initial surface and replaces a selection that left the list; a surface the user picked stays.
  // Waits for both the catalog and Settings: a surface only the fetched catalog has would
  // otherwise lose to the first entry of the static fallback.
  useEffect(() => {
    if (!catalogLoaded || configuredSurfaceId === undefined || httpApiSurfaces.length === 0) return
    if (!httpApiSurfaces.some((surface) => surface.surface_id === httpSurfaceId)) {
      const configured = providerSurfaceByIdFromList(httpApiSurfaces, configuredSurfaceId)
      setHttpSurfaceId((configured ?? httpApiSurfaces[0]).surface_id)
    }
  }, [catalogLoaded, configuredSurfaceId, httpApiSurfaces, httpSurfaceId])

  // Refresh token usage on mount
  useEffect(() => {
    ipc<{ totalInputTokens: number; totalOutputTokens: number; dailyBudget: number; budgetRemaining: number | null }>(
      'get_token_usage',
    )
      .then((r) => setTokenUsage({ total: r.totalInputTokens + r.totalOutputTokens, budget: r.dailyBudget || null }))
      .catch(() => {})
  }, [])

  // Fetch sessions list on mount
  useEffect(() => {
    ipc<SessionInfo[]>('list_ai_sessions')
      .then((items) => {
        setSessions(items)
        setSessionLoadError(null)
      })
      .catch((e) => {
        const message = errorMessage(e, t('chat.load_failed', 'Failed to load AI sessions.'))
        console.warn('list_ai_sessions failed:', e)
        setSessionLoadError(message)
        addToast('error', message, 5000)
      })
  }, [t])

  return {
    providerCatalog,
    httpApiSurfaces,
    httpSurfaceId,
    setHttpSurfaceId,
    sessions,
    setSessions,
    tokenUsage,
    sessionLoadError,
    setSessionLoadError,
  }
}
