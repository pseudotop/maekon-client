import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchProviderSurfaces } from '../../../api/client'
import type { ProviderSurfaceCatalog, ProviderSurfaceSpec } from '../../../api/contracts'
import { DEFAULT_PROVIDER_SURFACE_CATALOG } from '../../../api/defaultProviderSurfaceCatalog'
import { addToast } from '../../../hooks/useToast'
import type { SessionInfo } from '../types'
import { errorMessage, filterHttpApiSurfaces, ipc } from '../utils'

export function useSessionSetup() {
  const { t } = useTranslation()
  const [providerCatalog, setProviderCatalog] = useState<ProviderSurfaceCatalog>(DEFAULT_PROVIDER_SURFACE_CATALOG)
  const [httpSurfaceId, setHttpSurfaceId] = useState<string>('')
  const [sessions, setSessions] = useState<SessionInfo[]>([])
  const [tokenUsage, setTokenUsage] = useState<{ total: number; budget: number | null }>({ total: 0, budget: null })
  const [sessionLoadError, setSessionLoadError] = useState<string | null>(null)

  // Fetch provider catalog dynamically, fall back to static import
  useEffect(() => {
    fetchProviderSurfaces()
      .then(setProviderCatalog)
      .catch(() => {}) // keep static fallback
  }, [])

  const httpApiSurfaces: ProviderSurfaceSpec[] = useMemo(
    () => filterHttpApiSurfaces(providerCatalog),
    [providerCatalog],
  )

  // Set initial httpSurfaceId once surfaces are available
  useEffect(() => {
    if (httpApiSurfaces.length > 0 && !httpSurfaceId) {
      setHttpSurfaceId(httpApiSurfaces[0].surface_id)
    }
  }, [httpApiSurfaces, httpSurfaceId])

  // Keep httpSurfaceId in sync when surfaces change
  useEffect(() => {
    if (httpApiSurfaces.length === 0) return
    if (!httpApiSurfaces.some((surface) => surface.surface_id === httpSurfaceId)) {
      setHttpSurfaceId(httpApiSurfaces[0].surface_id)
    }
  }, [httpApiSurfaces, httpSurfaceId])

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
