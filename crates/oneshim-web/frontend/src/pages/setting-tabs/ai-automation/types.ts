import type {
  AppSettings,
  ExternalApiSettings,
  FeatureCapabilitySnapshot,
  OcrValidationSettings as OcrValidationSettingsType,
  ProviderEndpointProbeResult,
  ProviderSurfaceSpec,
  SandboxSettings,
  SceneActionOverrideSettings as SceneActionOverrideSettingsType,
  SceneIntelligenceSettings as SceneIntelligenceSettingsType,
} from '../../../api/client'
import type { EndpointSurfaceKind } from '../../../features/providerSurfaces'

export interface EndpointConfigProps {
  formData: AppSettings
  endpointKind: 'ocr_api' | 'llm_api'
  surface: ProviderSurfaceSpec | undefined
  usesNoAuth: boolean
  supportsModelSelection: boolean
  modelSupport: boolean | null
  modelCatalogNotice: string | null
  modelCompatibilityNotice: string | null
  modelCatalogLoading: 'ocr_api' | 'llm_api' | null
  endpointProbeResult: ProviderEndpointProbeResult | null
  endpointProbeLoading: boolean
  onExternalApiChange: (
    which: 'ocr_api' | 'llm_api',
    field: keyof ExternalApiSettings,
    value: string | number | boolean | null,
  ) => void
  onDiscoverModels: (which: 'ocr_api' | 'llm_api') => void
  getModelOptions: (which: 'ocr_api' | 'llm_api') => string[]
  canDiscoverModels: (which: 'ocr_api' | 'llm_api') => boolean
  showDirectApiFields: boolean
  showManagedHttpFields: boolean
  showSubprocessFields: boolean
}

export interface ProfileManagerProps {
  formData: AppSettings
  onSelectAiProviderProfile: (profileId: string | null) => void
  onSaveAiProviderProfile: (name: string) => void
  onDeleteAiProviderProfile: (profileId: string) => void
}

export interface SandboxConfigProps {
  formData: AppSettings
  onSandboxChange: (field: keyof SandboxSettings, value: boolean | string | number | string[]) => void
}

export interface SceneIntelligenceConfigProps {
  formData: AppSettings
  onSceneActionOverrideChange: (field: keyof SceneActionOverrideSettingsType, value: boolean | string | null) => void
  onSceneIntelligenceChange: (field: keyof SceneIntelligenceSettingsType, value: boolean | number) => void
  onOcrValidationChange: (field: keyof OcrValidationSettingsType, value: boolean | number) => void
}

export interface SurfaceStatusProps {
  surface: ProviderSurfaceSpec | undefined
  endpointKind: EndpointSurfaceKind
  featureCapabilities?: FeatureCapabilitySnapshot | null
  endpointProbeResult: ProviderEndpointProbeResult | null
  endpointProbeLoading: boolean
  usesCustomSelfHostedEndpoint: boolean
}
