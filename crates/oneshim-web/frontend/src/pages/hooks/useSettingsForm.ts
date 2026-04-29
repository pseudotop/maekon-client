import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  type AiProviderProfileConfig,
  type AiProviderSettings,
  type AppSettings,
  type AutomationSettings,
  discoverProviderModels,
  downloadBlob,
  type ExportDataType,
  type ExportFormat,
  type ExternalApiSettings,
  exportData,
  type MonitorControlSettings,
  type NotificationSettings as NotificationSettingsType,
  type OcrValidationSettings as OcrValidationSettingsType,
  type PrivacySettings as PrivacySettingsType,
  type ProviderDiscoveredModel,
  type ProviderModelsResponse,
  type ProviderSurfaceSpec,
  postUpdateAction,
  requestDesktopNotificationPermission,
  type SandboxSettings,
  type SavedAiProviderProfile,
  type SceneActionOverrideSettings as SceneActionOverrideSettingsType,
  type SceneIntelligenceSettings as SceneIntelligenceSettingsType,
  type ScheduleSettings as ScheduleSettingsType,
  type TelemetrySettings,
  type UpdateAction,
  updateSettings,
} from '../../api/client'
import {
  getCompatibleProviderSurfaces,
  providerSurfaceById,
  sortProviderSurfaces,
  surfaceKnownModel,
  surfaceUnknownModelPolicy,
} from '../../features/providerSurfaces'
import { useToast } from '../../hooks/useToast'
import {
  cloneAiProviderProfileConfig,
  isLlmModelCompatibilityUnknown,
  isLlmModelExplicitlyUnsupported,
  isOcrModelCompatibilityUnknown,
  isOcrModelExplicitlyUnsupported,
  modelDiscoverySensitiveField,
  modelDiscoverySignature,
  normalizeModelId,
  normalizeSavedProfileName,
  slugifySavedProfileId,
} from '../settings-utils'
import type { SettingsDataResult } from './useSettingsData'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface SettingsFormResult {
  formData: AppSettings | null
  setFormData: React.Dispatch<React.SetStateAction<AppSettings | null>>
  hasUnsavedChanges: boolean
  saveDisabled: boolean

  // Mutations
  saveMutation: { isPending: boolean }
  updateActionMutation: { isPending: boolean; mutate: (action: UpdateAction) => void }
  requestNotificationPermissionMutation: { isPending: boolean; mutate: () => void }

  // Export state
  exportFormat: ExportFormat
  setExportFormat: React.Dispatch<React.SetStateAction<ExportFormat>>
  exportLoading: ExportDataType | null

  // Model discovery state
  modelCatalog: Record<'ocr_api' | 'llm_api', string[]>
  modelCatalogDetails: Record<'ocr_api' | 'llm_api', ProviderDiscoveredModel[]>
  modelCatalogNotice: Record<'ocr_api' | 'llm_api', string | null>
  modelCatalogLoading: 'ocr_api' | 'llm_api' | null

  // Form handlers
  handleSubmit: (event: React.FormEvent<HTMLFormElement>) => void
  handleRevertChanges: () => void
  handleRootChange: (field: keyof AppSettings, value: number | boolean) => void
  handleNotificationChange: (field: keyof NotificationSettingsType, value: number | boolean) => void
  handleTelemetryChange: (field: keyof TelemetrySettings, value: boolean) => void
  handleMonitorChange: (field: keyof MonitorControlSettings, value: boolean) => void
  handlePrivacyChange: (field: keyof PrivacySettingsType, value: boolean | string | string[]) => void
  handleScheduleChange: (field: keyof ScheduleSettingsType, value: boolean | number | string[]) => void
  handleUpdateChange: (field: keyof AppSettings['update'], value: boolean | number | string) => void
  handleAutomationChange: (field: keyof AutomationSettings, value: boolean) => void
  handleSandboxChange: (field: keyof SandboxSettings, value: boolean | string | number | string[]) => void
  handleAiProviderChange: (
    field: keyof AiProviderSettings,
    value: string | boolean | ExternalApiSettings | OcrValidationSettingsType | SceneIntelligenceSettingsType | null,
  ) => void
  handleOcrValidationChange: (field: keyof OcrValidationSettingsType, value: boolean | number) => void
  handleSceneActionOverrideChange: (
    field: keyof SceneActionOverrideSettingsType,
    value: boolean | string | null,
  ) => void
  handleSceneIntelligenceChange: (field: keyof SceneIntelligenceSettingsType, value: boolean | number) => void
  handleExternalApiChange: (
    which: 'ocr_api' | 'llm_api',
    field: keyof ExternalApiSettings,
    value: string | number | boolean | null,
  ) => void
  handleProviderSurfaceChange: (which: 'ocr_api' | 'llm_api', nextSurfaceId: string) => void
  handleSelectAiProviderProfile: (profileId: string | null) => void
  handleSaveAiProviderProfile: (requestedName: string) => void
  handleDeleteAiProviderProfile: (profileId: string) => void
  handleExport: (dataType: ExportDataType) => void

  // Model-related helpers
  resolveEndpointSurface: (which: 'ocr_api' | 'llm_api') => ProviderSurfaceSpec | undefined
  getCompatibleSurfaceOptions: (which: 'ocr_api' | 'llm_api') => ProviderSurfaceSpec[]
  getModelOptions: (which: 'ocr_api' | 'llm_api') => string[]
  getModelCompatibilityNotice: (which: 'ocr_api' | 'llm_api') => string | null
  canDiscoverModels: (which: 'ocr_api' | 'llm_api') => boolean
  discoverModels: (which: 'ocr_api' | 'llm_api') => void
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

export function useSettingsForm(data: SettingsDataResult): SettingsFormResult {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const { show: showToast } = useToast()

  const {
    settings,
    featureCapabilities,
    secretBackendCapabilities,
    providerCatalog,
    defaultByokBackendKind,
    normalizeEndpointSettings,
    defaultExternalApiSettings,
    resolveSurfaceForState,
  } = data

  // ---- Form state ---------------------------------------------------------
  const [formData, setFormData] = useState<AppSettings | null>(null)
  const formDataRef = useRef<AppSettings | null>(null)
  const lastLoadedSettingsRef = useRef<string | null>(null)

  // ---- Export state -------------------------------------------------------
  const [exportFormat, setExportFormat] = useState<ExportFormat>('json')
  const [exportLoading, setExportLoading] = useState<ExportDataType | null>(null)

  // ---- Model discovery state ----------------------------------------------
  const [modelCatalog, setModelCatalog] = useState<Record<'ocr_api' | 'llm_api', string[]>>({
    ocr_api: [],
    llm_api: [],
  })
  const [modelCatalogDetails, setModelCatalogDetails] = useState<
    Record<'ocr_api' | 'llm_api', ProviderDiscoveredModel[]>
  >({
    ocr_api: [],
    llm_api: [],
  })
  const [modelCatalogNotice, setModelCatalogNotice] = useState<Record<'ocr_api' | 'llm_api', string | null>>({
    ocr_api: null,
    llm_api: null,
  })
  const [modelCatalogLoading, setModelCatalogLoading] = useState<'ocr_api' | 'llm_api' | null>(null)

  // ---- Normalization helpers (memoised) -----------------------------------
  const normalizeAiProviderProfileConfig = useCallback(
    (config: AiProviderProfileConfig): AiProviderProfileConfig => {
      const next = cloneAiProviderProfileConfig(config)
      if (next.ocr_provider === 'Remote') {
        next.ocr_api = normalizeEndpointSettings(next.access_mode, 'ocr_api', next.ocr_api)
      }
      if (next.llm_provider === 'Remote') {
        next.llm_api = normalizeEndpointSettings(next.access_mode, 'llm_api', next.llm_api)
      }
      return next
    },
    [normalizeEndpointSettings],
  )

  const normalizeSavedProfiles = useCallback(
    (profiles: SavedAiProviderProfile[] | null | undefined): SavedAiProviderProfile[] =>
      (profiles ?? []).map((profile) => ({
        profile_id: profile.profile_id,
        name: normalizeSavedProfileName(profile.name),
        ai_provider: normalizeAiProviderProfileConfig(profile.ai_provider),
        updated_at: profile.updated_at ?? null,
      })),
    [normalizeAiProviderProfileConfig],
  )

  const sanitizeLoadedSettings = useCallback(
    (incoming: AppSettings): AppSettings => {
      const normalizedProfiles = normalizeSavedProfiles(incoming.ai_provider.saved_profiles)
      const aiProvider = {
        ...normalizeAiProviderProfileConfig(incoming.ai_provider),
        active_profile_id: normalizedProfiles.some(
          (profile) => profile.profile_id === incoming.ai_provider.active_profile_id,
        )
          ? (incoming.ai_provider.active_profile_id ?? null)
          : null,
        saved_profiles: normalizedProfiles,
      }
      return { ...incoming, ai_provider: aiProvider }
    },
    [normalizeAiProviderProfileConfig, normalizeSavedProfiles],
  )

  const markAiProviderAsCustom = useCallback(
    (aiProvider: AiProviderSettings): AiProviderSettings => {
      const savedProfiles = normalizeSavedProfiles(aiProvider.saved_profiles)
      return {
        ...aiProvider,
        active_profile_id: savedProfiles.some((profile) => profile.profile_id === aiProvider.active_profile_id)
          ? (aiProvider.active_profile_id ?? null)
          : null,
        saved_profiles: savedProfiles,
      }
    },
    [normalizeSavedProfiles],
  )

  const createSavedAiProviderProfile = useCallback(
    (
      currentAiProvider: AiProviderSettings,
      existingProfiles: SavedAiProviderProfile[],
      requestedName: string,
    ): SavedAiProviderProfile | null => {
      const normalizedName = normalizeSavedProfileName(requestedName)
      if (!normalizedName) return null

      const activeProfile = currentAiProvider.active_profile_id
        ? existingProfiles.find((profile) => profile.profile_id === currentAiProvider.active_profile_id)
        : undefined
      const matchedByName = existingProfiles.find(
        (profile) => profile.name.localeCompare(normalizedName, undefined, { sensitivity: 'base' }) === 0,
      )
      const profileId =
        activeProfile?.name === normalizedName
          ? activeProfile.profile_id
          : (matchedByName?.profile_id ?? slugifySavedProfileId(normalizedName))

      const usedIds = new Set(
        existingProfiles.filter((profile) => profile.profile_id !== profileId).map((profile) => profile.profile_id),
      )
      let nextProfileId = profileId
      let suffix = 2
      while (usedIds.has(nextProfileId)) {
        nextProfileId = `${profileId}-${suffix}`
        suffix += 1
      }

      return {
        profile_id: nextProfileId,
        name: normalizedName,
        ai_provider: normalizeAiProviderProfileConfig(currentAiProvider),
        updated_at: new Date().toISOString(),
      }
    },
    [normalizeAiProviderProfileConfig],
  )

  // ---- Sync formDataRef ---------------------------------------------------
  useEffect(() => {
    formDataRef.current = formData
  }, [formData])

  // ---- Sync settings → formData -------------------------------------------
  useEffect(() => {
    if (settings) {
      const sanitized = sanitizeLoadedSettings(settings)
      const serialized = JSON.stringify(sanitized)
      setFormData((current) => {
        if (!current) return sanitized
        if (lastLoadedSettingsRef.current && JSON.stringify(current) === lastLoadedSettingsRef.current) {
          return sanitized
        }
        return current
      })
      lastLoadedSettingsRef.current = serialized
    }
  }, [sanitizeLoadedSettings, settings])

  // ---- Apply secret backend defaults when capabilities arrive -------------
  useEffect(() => {
    if (!secretBackendCapabilities) return

    setFormData((current) => {
      if (!current) return current

      let changed = false
      const applyBackendDefault = (endpoint: ExternalApiSettings | null): ExternalApiSettings | null => {
        if (!endpoint) return endpoint
        if (endpoint.backend_kind !== 'unavailable') return endpoint
        if (endpoint.has_secret || endpoint.api_key_masked.trim().length > 0) return endpoint
        changed = true
        return {
          ...endpoint,
          backend_kind: defaultByokBackendKind,
          can_edit_secret:
            defaultByokBackendKind !== 'env' &&
            defaultByokBackendKind !== 'bridge_managed' &&
            defaultByokBackendKind !== 'unavailable',
        }
      }

      const nextOcr = applyBackendDefault(current.ai_provider.ocr_api)
      const nextLlm = applyBackendDefault(current.ai_provider.llm_api)
      if (!changed) return current

      return {
        ...current,
        ai_provider: { ...current.ai_provider, ocr_api: nextOcr, llm_api: nextLlm },
      }
    })
  }, [defaultByokBackendKind, secretBackendCapabilities])

  // ---- Model discovery reset helper ---------------------------------------
  const resetModelDiscoveryState = (targets: Array<'ocr_api' | 'llm_api'>) => {
    setModelCatalog((current) => {
      const next = { ...current }
      for (const target of targets) next[target] = []
      return next
    })
    setModelCatalogDetails((current) => {
      const next = { ...current }
      for (const target of targets) next[target] = []
      return next
    })
    setModelCatalogNotice((current) => {
      const next = { ...current }
      for (const target of targets) next[target] = null
      return next
    })
    setModelCatalogLoading((current) => (current && targets.includes(current) ? null : current))
  }

  // ---- Access mode defaults -----------------------------------------------
  const applyAccessModeDefaults = (
    currentAiProvider: AiProviderSettings,
    nextAccessMode: string,
  ): AiProviderSettings => {
    const nextAiProvider: AiProviderSettings = { ...currentAiProvider, access_mode: nextAccessMode }

    if (nextAccessMode === 'ProviderSubscriptionCli') {
      nextAiProvider.llm_provider = 'Remote'
      nextAiProvider.llm_api = normalizeEndpointSettings(nextAccessMode, 'llm_api', nextAiProvider.llm_api)
      if (nextAiProvider.ocr_provider === 'Remote') {
        nextAiProvider.ocr_api = normalizeEndpointSettings(nextAccessMode, 'ocr_api', nextAiProvider.ocr_api)
      }
      return nextAiProvider
    }

    if (nextAccessMode === 'ProviderOAuth') {
      if (nextAiProvider.llm_provider === 'Remote') {
        nextAiProvider.llm_api = normalizeEndpointSettings(nextAccessMode, 'llm_api', nextAiProvider.llm_api)
      }
      if (nextAiProvider.ocr_provider === 'Remote') {
        nextAiProvider.ocr_api = normalizeEndpointSettings(nextAccessMode, 'ocr_api', nextAiProvider.ocr_api)
      }
      return nextAiProvider
    }

    if (nextAiProvider.ocr_provider === 'Remote') {
      nextAiProvider.ocr_api = normalizeEndpointSettings(nextAccessMode, 'ocr_api', nextAiProvider.ocr_api)
    }
    if (nextAiProvider.llm_provider === 'Remote') {
      nextAiProvider.llm_api = normalizeEndpointSettings(nextAccessMode, 'llm_api', nextAiProvider.llm_api)
    }
    return nextAiProvider
  }

  // ---- Mutations ----------------------------------------------------------
  const saveMutation = useMutation({
    mutationFn: updateSettings,
    onSuccess: (savedSettings) => {
      queryClient.setQueryData(['settings'], savedSettings)
      queryClient.invalidateQueries({ queryKey: ['settings'] })
      const sanitized = sanitizeLoadedSettings(savedSettings)
      lastLoadedSettingsRef.current = JSON.stringify(sanitized)
      setFormData(sanitized)
      showToast('success', t('settings.savedFull'), 5000)
    },
    onError: (error: Error) => {
      showToast('error', error.message, 5000)
    },
  })

  const updateActionMutation = useMutation({
    mutationFn: (action: UpdateAction) => postUpdateAction(action),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['update-status'] })
      showToast('success', t('settings.updateActionSuccess'), 3000)
    },
    onError: (error: Error) => {
      showToast('error', error.message, 5000)
    },
  })

  const requestNotificationPermissionMutation = useMutation({
    mutationFn: requestDesktopNotificationPermission,
    onSuccess: (snapshot) => {
      queryClient.setQueryData(['desktop-permission-status'], snapshot)
      if (snapshot.notifications.state === 'granted') {
        showToast(
          'success',
          t('settings.permissionNotificationRequestGranted', 'Notifications are ready for Maekon.'),
          3000,
        )
      } else {
        showToast(
          'info',
          t(
            'settings.permissionNotificationRequestFollowUp',
            'Check the macOS notification prompt or System Settings, then refresh the status if needed.',
          ),
          4000,
        )
      }
    },
    onError: (error: Error) => {
      showToast('error', error.message, 5000)
    },
  })

  // ---- Dirty / save state -------------------------------------------------
  const serializedFormData = formData ? JSON.stringify(formData) : null
  const hasUnsavedChanges = Boolean(
    settings && formData && serializedFormData && serializedFormData !== lastLoadedSettingsRef.current,
  )
  const saveDisabled = !settings || !formData || saveMutation.isPending || !hasUnsavedChanges

  // ---- Form handlers ------------------------------------------------------
  const handleSubmit = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!formData) return
    saveMutation.mutate(formData)
  }

  const handleRevertChanges = () => {
    const lastLoaded = lastLoadedSettingsRef.current
    if (!lastLoaded) return
    const parsed = JSON.parse(lastLoaded) as AppSettings
    setFormData(parsed)
  }

  const handleRootChange = (field: keyof AppSettings, value: number | boolean) => {
    setFormData((current) => (current ? { ...current, [field]: value } : current))
  }

  const handleNotificationChange = (field: keyof NotificationSettingsType, value: number | boolean) => {
    setFormData((current) =>
      current ? { ...current, notification: { ...current.notification, [field]: value } } : current,
    )
  }

  const handleTelemetryChange = (field: keyof TelemetrySettings, value: boolean) => {
    setFormData((current) => (current ? { ...current, telemetry: { ...current.telemetry, [field]: value } } : current))
  }

  const handleMonitorChange = (field: keyof MonitorControlSettings, value: boolean) => {
    setFormData((current) => (current ? { ...current, monitor: { ...current.monitor, [field]: value } } : current))
  }

  const handlePrivacyChange = (field: keyof PrivacySettingsType, value: boolean | string | string[]) => {
    setFormData((current) => (current ? { ...current, privacy: { ...current.privacy, [field]: value } } : current))
  }

  const handleScheduleChange = (field: keyof ScheduleSettingsType, value: boolean | number | string[]) => {
    setFormData((current) => (current ? { ...current, schedule: { ...current.schedule, [field]: value } } : current))
  }

  const handleUpdateChange = (field: keyof AppSettings['update'], value: boolean | number | string) => {
    setFormData((current) => (current ? { ...current, update: { ...current.update, [field]: value } } : current))
  }

  const handleAutomationChange = (field: keyof AutomationSettings, value: boolean) => {
    setFormData((current) =>
      current ? { ...current, automation: { ...current.automation, [field]: value } } : current,
    )
  }

  const handleSandboxChange = (field: keyof SandboxSettings, value: boolean | string | number | string[]) => {
    setFormData((current) => (current ? { ...current, sandbox: { ...current.sandbox, [field]: value } } : current))
  }

  const handleAiProviderChange = (
    field: keyof AiProviderSettings,
    value: string | boolean | ExternalApiSettings | OcrValidationSettingsType | SceneIntelligenceSettingsType | null,
  ) => {
    if (field === 'access_mode' && typeof value === 'string') {
      resetModelDiscoveryState(['ocr_api', 'llm_api'])
    }
    if (field === 'ocr_provider' && value === 'Remote') {
      resetModelDiscoveryState(['ocr_api'])
    }
    if (field === 'llm_provider' && value === 'Remote') {
      resetModelDiscoveryState(['llm_api'])
    }

    setFormData((current) =>
      current
        ? (() => {
            if (field === 'access_mode' && typeof value === 'string') {
              return {
                ...current,
                ai_provider: markAiProviderAsCustom(applyAccessModeDefaults(current.ai_provider, value)),
              }
            }

            const nextAiProvider = { ...current.ai_provider, [field]: value }

            if (field === 'ocr_provider' && value === 'Remote') {
              nextAiProvider.ocr_api = normalizeEndpointSettings(
                current.ai_provider.access_mode,
                'ocr_api',
                nextAiProvider.ocr_api,
              )
            }

            if (field === 'llm_provider' && value === 'Remote') {
              nextAiProvider.llm_api = normalizeEndpointSettings(
                current.ai_provider.access_mode,
                'llm_api',
                nextAiProvider.llm_api,
              )
            }

            return { ...current, ai_provider: markAiProviderAsCustom(nextAiProvider) }
          })()
        : current,
    )
  }

  const handleOcrValidationChange = (field: keyof OcrValidationSettingsType, value: boolean | number) => {
    setFormData((current) =>
      current
        ? {
            ...current,
            ai_provider: markAiProviderAsCustom({
              ...current.ai_provider,
              ocr_validation: { ...current.ai_provider.ocr_validation, [field]: value },
            }),
          }
        : current,
    )
  }

  const handleSceneActionOverrideChange = (
    field: keyof SceneActionOverrideSettingsType,
    value: boolean | string | null,
  ) => {
    setFormData((current) =>
      current
        ? {
            ...current,
            ai_provider: markAiProviderAsCustom({
              ...current.ai_provider,
              scene_action_override: { ...current.ai_provider.scene_action_override, [field]: value },
            }),
          }
        : current,
    )
  }

  const handleSceneIntelligenceChange = (field: keyof SceneIntelligenceSettingsType, value: boolean | number) => {
    setFormData((current) =>
      current
        ? {
            ...current,
            ai_provider: markAiProviderAsCustom({
              ...current.ai_provider,
              scene_intelligence: { ...current.ai_provider.scene_intelligence, [field]: value },
            }),
          }
        : current,
    )
  }

  const handleExternalApiChange = (
    which: 'ocr_api' | 'llm_api',
    field: keyof ExternalApiSettings,
    value: string | number | boolean | null,
  ) => {
    if (modelDiscoverySensitiveField(field)) {
      resetModelDiscoveryState([which])
    }
    setFormData((current) => {
      if (!current) return current
      const existing = current.ai_provider[which] ?? defaultExternalApiSettings(current.ai_provider.access_mode, which)
      return {
        ...current,
        ai_provider: markAiProviderAsCustom({
          ...current.ai_provider,
          [which]: { ...existing, [field]: value },
        }),
      }
    })
  }

  const handleProviderSurfaceChange = (which: 'ocr_api' | 'llm_api', nextSurfaceId: string) => {
    const nextSurface = providerSurfaceById(providerCatalog, nextSurfaceId)
    if (!nextSurface) return

    resetModelDiscoveryState([which])
    setFormData((current) => {
      if (!current) return current
      const existing = current.ai_provider[which] ?? defaultExternalApiSettings(current.ai_provider.access_mode, which)
      return {
        ...current,
        ai_provider: markAiProviderAsCustom({
          ...current.ai_provider,
          [which]: normalizeEndpointSettings(current.ai_provider.access_mode, which, existing, nextSurface),
        }),
      }
    })
  }

  const handleSelectAiProviderProfile = (profileId: string | null) => {
    resetModelDiscoveryState(['ocr_api', 'llm_api'])
    setFormData((current) => {
      if (!current) return current

      const savedProfiles = normalizeSavedProfiles(current.ai_provider.saved_profiles)
      if (!profileId) {
        return {
          ...current,
          ai_provider: { ...current.ai_provider, active_profile_id: null, saved_profiles: savedProfiles },
        }
      }

      const selectedProfile = savedProfiles.find((profile) => profile.profile_id === profileId)
      if (!selectedProfile) {
        return {
          ...current,
          ai_provider: { ...current.ai_provider, active_profile_id: null, saved_profiles: savedProfiles },
        }
      }

      return {
        ...current,
        ai_provider: {
          ...normalizeAiProviderProfileConfig(selectedProfile.ai_provider),
          active_profile_id: selectedProfile.profile_id,
          saved_profiles: savedProfiles,
        },
      }
    })
  }

  const handleSaveAiProviderProfile = (requestedName: string) => {
    let savedProfileName: string | null = null
    setFormData((current) => {
      if (!current) return current

      const savedProfiles = normalizeSavedProfiles(current.ai_provider.saved_profiles)
      const nextProfile = createSavedAiProviderProfile(current.ai_provider, savedProfiles, requestedName)
      if (!nextProfile) return current

      const nextProfiles = [
        ...savedProfiles.filter((profile) => profile.profile_id !== nextProfile.profile_id),
        nextProfile,
      ].sort((left, right) => left.name.localeCompare(right.name, undefined, { sensitivity: 'base' }))
      savedProfileName = nextProfile.name

      return {
        ...current,
        ai_provider: {
          ...current.ai_provider,
          active_profile_id: nextProfile.profile_id,
          saved_profiles: nextProfiles,
        },
      }
    })

    if (savedProfileName) {
      showToast('success', t('settingsAutomation.profileSavedSuccess', { name: savedProfileName }), 3000)
    } else {
      showToast('error', t('settingsAutomation.profileNameRequired'), 4000)
    }
  }

  const handleDeleteAiProviderProfile = (profileId: string) => {
    let deletedProfileName: string | null = null
    setFormData((current) => {
      if (!current) return current

      const savedProfiles = normalizeSavedProfiles(current.ai_provider.saved_profiles)
      const profileToDelete = savedProfiles.find((profile) => profile.profile_id === profileId)
      if (!profileToDelete) return current

      deletedProfileName = profileToDelete.name
      return {
        ...current,
        ai_provider: {
          ...current.ai_provider,
          active_profile_id:
            current.ai_provider.active_profile_id === profileId
              ? null
              : (current.ai_provider.active_profile_id ?? null),
          saved_profiles: savedProfiles.filter((profile) => profile.profile_id !== profileId),
        },
      }
    })

    if (deletedProfileName) {
      showToast('success', t('settingsAutomation.profileDeletedSuccess', { name: deletedProfileName }), 3000)
    }
  }

  // ---- Surface / model helpers --------------------------------------------
  const resolveEndpointSurface = (which: 'ocr_api' | 'llm_api'): ProviderSurfaceSpec | undefined =>
    resolveSurfaceForState(formData, which)

  const getCompatibleSurfaceOptions = (which: 'ocr_api' | 'llm_api'): ProviderSurfaceSpec[] =>
    sortProviderSurfaces(
      getCompatibleProviderSurfaces(providerCatalog, formData?.ai_provider.access_mode, which, featureCapabilities),
      featureCapabilities,
    )

  const getSurfaceModels = (which: 'ocr_api' | 'llm_api'): string[] => {
    const surface = resolveEndpointSurface(which)
    if (!surface) return []
    return which === 'ocr_api' ? (surface.default_models.ocr_models ?? []) : (surface.default_models.llm_models ?? [])
  }

  const ocrSurface = resolveEndpointSurface('ocr_api')

  const findModelDetail = (
    which: 'ocr_api' | 'llm_api',
    modelId: string | null | undefined,
  ): ProviderDiscoveredModel | undefined => {
    const normalized = normalizeModelId(modelId)
    if (!normalized) return undefined
    return modelCatalogDetails[which].find((detail) => normalizeModelId(detail.id) === normalized)
  }

  const getModelOptions = (which: 'ocr_api' | 'llm_api'): string[] => {
    const surfaceModels = getSurfaceModels(which)
    const surface = resolveEndpointSurface(which)
    const unknownPolicy = surfaceUnknownModelPolicy(surface, which)
    const isAllowedDiscoveredModel = (detail: ProviderDiscoveredModel): boolean => {
      if (which === 'ocr_api') {
        if (isOcrModelExplicitlyUnsupported(detail, ocrSurface)) return false
        return !(unknownPolicy === 'reject' && isOcrModelCompatibilityUnknown(detail, ocrSurface))
      }
      if (isLlmModelExplicitlyUnsupported(detail)) return false
      return !(unknownPolicy === 'reject' && isLlmModelCompatibilityUnknown(detail))
    }
    const discoveredModels =
      modelCatalogDetails[which].length > 0
        ? modelCatalogDetails[which].filter((detail) => isAllowedDiscoveredModel(detail)).map((detail) => detail.id)
        : modelCatalog[which]
    const allowedSurfaceModels =
      which === 'ocr_api'
        ? surfaceModels.filter((model) => {
            const detail = findModelDetail(which, model)
            return !detail || isAllowedDiscoveredModel(detail)
          })
        : surfaceModels.filter((model) => {
            const detail = findModelDetail(which, model)
            return !detail || isAllowedDiscoveredModel(detail)
          })
    return Array.from(new Set([...discoveredModels, ...allowedSurfaceModels]))
  }

  const getModelCompatibilityNotice = (which: 'ocr_api' | 'llm_api'): string | null => {
    const currentModel = formData?.ai_provider[which]?.model
    const surface = resolveEndpointSurface(which)
    const unknownPolicy = surfaceUnknownModelPolicy(surface, which)
    const detail = findModelDetail(which, currentModel)
    if (!detail) {
      if (currentModel?.trim() && !surfaceKnownModel(surface, currentModel)) {
        if (unknownPolicy === 'reject') {
          return which === 'ocr_api'
            ? t('settingsAutomation.ocrModelCompatibilityUnknownRejected', { model: currentModel })
            : t('settingsAutomation.llmModelCompatibilityUnknownRejected', { model: currentModel })
        }
        if (unknownPolicy === 'warn') {
          return which === 'ocr_api'
            ? t('settingsAutomation.ocrModelCompatibilityUnknown', { model: currentModel })
            : t('settingsAutomation.llmModelCompatibilityUnknown', { model: currentModel })
        }
      }
      return null
    }
    if (which === 'ocr_api' && isOcrModelExplicitlyUnsupported(detail, ocrSurface)) {
      return t('settingsAutomation.ocrModelUnsupported', { model: detail.display_name ?? detail.id })
    }
    if (which === 'ocr_api' && isOcrModelCompatibilityUnknown(detail, ocrSurface)) {
      if (unknownPolicy === 'reject') {
        return t('settingsAutomation.ocrModelCompatibilityUnknownRejected', {
          model: detail.display_name ?? detail.id,
        })
      }
      if (unknownPolicy === 'warn') {
        return t('settingsAutomation.ocrModelCompatibilityUnknown', { model: detail.display_name ?? detail.id })
      }
    }
    if (which === 'llm_api' && isLlmModelExplicitlyUnsupported(detail)) {
      return t('settingsAutomation.llmModelUnsupported', { model: detail.display_name ?? detail.id })
    }
    if (which === 'llm_api' && isLlmModelCompatibilityUnknown(detail)) {
      if (unknownPolicy === 'reject') {
        return t('settingsAutomation.llmModelCompatibilityUnknownRejected', {
          model: detail.display_name ?? detail.id,
        })
      }
      if (unknownPolicy === 'warn') {
        return t('settingsAutomation.llmModelCompatibilityUnknown', { model: detail.display_name ?? detail.id })
      }
    }
    return null
  }

  const canDiscoverModelsCheck = (which: 'ocr_api' | 'llm_api'): boolean => {
    const surface = resolveEndpointSurface(which)
    if (!surface) return false
    const transport = surface?.model_catalog_transport
    if (!transport) return surface.supports.model_catalog
    return which === 'ocr_api' ? transport.ocr_supported : transport.llm_supported
  }

  // ---- Model discovery result handler -------------------------------------
  const handleModelDiscoveryResult = (
    which: 'ocr_api' | 'llm_api',
    currentModel: string | null | undefined,
    requestSignature: string,
    result: ProviderModelsResponse,
  ) => {
    const latestSignature = modelDiscoverySignature(formDataRef.current?.ai_provider[which])
    if (latestSignature !== requestSignature) return

    setModelCatalog((current) => ({ ...current, [which]: result.models }))
    setModelCatalogNotice((current) => ({
      ...current,
      [which]: result.notice ?? (result.models.length === 0 ? t('settingsAutomation.modelDiscoveryNoModels') : null),
    }))
    setModelCatalogDetails((current) => ({ ...current, [which]: result.model_details ?? [] }))

    const unknownPolicy = surfaceUnknownModelPolicy(resolveEndpointSurface(which), which)
    const preferredDiscoveredModel =
      which === 'ocr_api'
        ? (result.model_details ?? []).find(
            (detail) =>
              !isOcrModelExplicitlyUnsupported(detail, ocrSurface) &&
              !(unknownPolicy === 'reject' && isOcrModelCompatibilityUnknown(detail, ocrSurface)),
          )?.id
        : (result.model_details ?? []).find(
            (detail) =>
              !isLlmModelExplicitlyUnsupported(detail) &&
              !(unknownPolicy === 'reject' && isLlmModelCompatibilityUnknown(detail)),
          )?.id

    const canFallbackToRawModelList = !result.model_details || result.model_details.length === 0
    if (
      (!currentModel || !currentModel.trim()) &&
      (preferredDiscoveredModel ||
        (canFallbackToRawModelList && unknownPolicy !== 'reject' && result.models.length > 0))
    ) {
      handleExternalApiChange(which, 'model', preferredDiscoveredModel ?? result.models[0])
    }
  }

  // ---- Discover models ----------------------------------------------------
  const discoverModelsAsync = async (which: 'ocr_api' | 'llm_api') => {
    if (!formData) return
    const current = formData.ai_provider[which]
    if (!current) {
      showToast('error', t('settingsAutomation.modelDiscoveryMissingConfig'), 5000)
      return
    }
    if (!canDiscoverModelsCheck(which)) {
      showToast('error', t('settingsAutomation.modelDiscoveryUnsupportedSurface'), 5000)
      return
    }
    const surface = resolveEndpointSurface(which)
    const usesNoAuth =
      which === 'ocr_api'
        ? surface?.ocr_transport?.auth_scheme === 'none'
        : surface?.llm_transport?.auth_scheme === 'none'
    const useSavedSecret = current.has_secret && !current.api_key_masked?.trim()
    if (!usesNoAuth && !current.api_key_masked?.trim() && !useSavedSecret) {
      showToast('error', t('settingsAutomation.modelDiscoveryMissingKey'), 5000)
      return
    }

    setModelCatalogLoading(which)
    try {
      const requestSignature = modelDiscoverySignature(current)
      const result = await discoverProviderModels({
        provider_type: current.provider_type ?? 'Generic',
        api_key: current.api_key_masked,
        endpoint: current.endpoint || null,
        surface: which,
        surface_id: current.surface_id || null,
        use_saved_secret: useSavedSecret,
      })
      handleModelDiscoveryResult(which, current.model, requestSignature, result)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      setModelCatalog((currentCatalog) => ({ ...currentCatalog, [which]: [] }))
      setModelCatalogDetails((currentDetails) => ({ ...currentDetails, [which]: [] }))
      setModelCatalogNotice((currentNotice) => ({ ...currentNotice, [which]: message }))
      showToast('error', message, 5000)
    } finally {
      setModelCatalogLoading(null)
    }
  }

  // ---- Export handler -----------------------------------------------------
  const handleExport = async (dataType: ExportDataType) => {
    setExportLoading(dataType)
    try {
      const to = new Date().toISOString()
      const from = new Date(Date.now() - 7 * 24 * 60 * 60 * 1000).toISOString()
      const blob = await exportData(dataType, exportFormat, from, to)
      const ext = exportFormat === 'csv' ? 'csv' : 'json'
      const timestamp = new Date().toISOString().split('T')[0]
      downloadBlob(blob, `${dataType}_${timestamp}.${ext}`)
      showToast('success', t('settings.exportDone'), 3000)
    } catch (error) {
      showToast('error', `${t('settings.saveFailed')}: ${error instanceof Error ? error.message : String(error)}`, 5000)
    } finally {
      setExportLoading(null)
    }
  }

  // ---- Return value -------------------------------------------------------
  return {
    formData,
    setFormData,
    hasUnsavedChanges,
    saveDisabled,

    saveMutation: { isPending: saveMutation.isPending },
    updateActionMutation: { isPending: updateActionMutation.isPending, mutate: updateActionMutation.mutate },
    requestNotificationPermissionMutation: {
      isPending: requestNotificationPermissionMutation.isPending,
      mutate: requestNotificationPermissionMutation.mutate,
    },

    exportFormat,
    setExportFormat,
    exportLoading,

    modelCatalog,
    modelCatalogDetails,
    modelCatalogNotice,
    modelCatalogLoading,

    handleSubmit,
    handleRevertChanges,
    handleRootChange,
    handleNotificationChange,
    handleTelemetryChange,
    handleMonitorChange,
    handlePrivacyChange,
    handleScheduleChange,
    handleUpdateChange,
    handleAutomationChange,
    handleSandboxChange,
    handleAiProviderChange,
    handleOcrValidationChange,
    handleSceneActionOverrideChange,
    handleSceneIntelligenceChange,
    handleExternalApiChange,
    handleProviderSurfaceChange,
    handleSelectAiProviderProfile,
    handleSaveAiProviderProfile,
    handleDeleteAiProviderProfile,
    handleExport: (dataType: ExportDataType) => void handleExport(dataType),

    resolveEndpointSurface,
    getCompatibleSurfaceOptions,
    getModelOptions,
    getModelCompatibilityNotice,
    canDiscoverModels: canDiscoverModelsCheck,
    discoverModels: (which: 'ocr_api' | 'llm_api') => void discoverModelsAsync(which),
  }
}
