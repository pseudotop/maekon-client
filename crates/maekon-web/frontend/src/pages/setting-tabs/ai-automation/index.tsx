import { useTranslation } from 'react-i18next'
import type { ProviderSurfaceSpec } from '../../../api/client'
import { Badge, Button, Card, CardTitle, FieldHint, GuidancePanel, Select } from '../../../components/ui'
import {
  findFeatureCapability,
  maturityBadgeColor,
  providerSurfaceAvailability,
  providerSurfaceMaturity,
  providerSurfaceStatusCopyKey,
} from '../../../features/featureCapabilities'
import {
  defaultSurfaceEndpoint,
  type EndpointSurfaceKind,
  preferredRelatedProviderSurfaceFromList,
  surfaceModelSupportsCapability,
  surfaceOcrExecutionStrategy,
  surfaceSupportsModelSelection,
} from '../../../features/providerSurfaces'
import { form, typography } from '../../../styles/tokens'
import { useLoadedFormData, useSettingsFormContext } from '../../settings/SettingsFormContext'
import {
  executionKindLabel,
  placementKindDescription,
  placementKindLabel,
  requirementLabel,
  surfaceUsesNoAuth,
} from '../ai-automation-utils'
import OAuthConnectionPanel from '../OAuthConnectionPanel'
import { isProviderOAuthAccessMode } from '../oauth-panel-support'
import ProviderWizard, { type ProviderDef } from '../ProviderWizard'
import ToggleRow from '../ToggleRow'
import LlmEndpointConfig from './LlmEndpointConfig'
import OcrEndpointConfig from './OcrEndpointConfig'
import ProfileManager from './ProfileManager'
import SandboxConfig from './SandboxConfig'
import SceneIntelligenceConfig from './SceneIntelligenceConfig'

export default function AiAutomationTab() {
  const { form: settingsForm, data } = useSettingsFormContext()
  const { t } = useTranslation()
  const formData = useLoadedFormData()
  const allProviderSurfaces = data.providerCatalog.surfaces
  const featureCapabilities = data.featureCapabilities
  const secretBackendCapabilities = data.secretBackendCapabilities
  const providerSurfaceOptions = {
    ocr_api: settingsForm.getCompatibleSurfaceOptions('ocr_api'),
    llm_api: settingsForm.getCompatibleSurfaceOptions('llm_api'),
  }
  const modelCatalogNotice = settingsForm.modelCatalogNotice
  const modelCompatibilityNotice = {
    ocr_api: settingsForm.getModelCompatibilityNotice('ocr_api'),
    llm_api: settingsForm.getModelCompatibilityNotice('llm_api'),
  }
  const modelCatalogLoading = settingsForm.modelCatalogLoading
  const endpointProbeResult = {
    ocr_api: data.ocrEndpointProbe,
    llm_api: data.llmEndpointProbe,
  }
  const endpointProbeLoading = {
    ocr_api: data.ocrEndpointProbeLoading,
    llm_api: data.llmEndpointProbeLoading,
  }

  const currentOcrSurface = settingsForm.resolveEndpointSurface('ocr_api')
  const currentLlmSurface = settingsForm.resolveEndpointSurface('llm_api')
  const isCliAccessMode = formData.ai_provider.access_mode === 'ProviderSubscriptionCli'
  const isOAuthAccessMode = isProviderOAuthAccessMode(formData.ai_provider.access_mode)
  const showOcrRemoteSection = formData.ai_provider.ocr_provider === 'Remote'
  const showLlmSurfaceSection = formData.ai_provider.llm_provider === 'Remote' || isCliAccessMode
  const currentLlmFeature = currentLlmSurface
    ? findFeatureCapability(featureCapabilities, currentLlmSurface.surface_id)
    : null
  const currentLlmMaturity = providerSurfaceMaturity(currentLlmSurface, featureCapabilities)
  const currentLlmRequirements = currentLlmFeature?.requires ?? []
  const oauthPanels = [
    { endpointKind: 'llm_api' as const, surface: currentLlmSurface },
    { endpointKind: 'ocr_api' as const, surface: currentOcrSurface },
  ].flatMap((entry) => {
    if (!entry.surface || entry.surface.execution_kind !== 'managed_http') {
      return []
    }

    const preferredCliSurface = preferredRelatedProviderSurfaceFromList(
      allProviderSurfaces,
      entry.surface,
      'subprocess_cli',
      featureCapabilities,
    )
    const preferredCliAvailability = providerSurfaceAvailability(preferredCliSurface, featureCapabilities)
    return [
      {
        endpointKind: entry.endpointKind,
        oauthSurface: entry.surface,
        preferredCliSurface,
        showPreferredCliCta:
          Boolean(preferredCliSurface) &&
          preferredCliAvailability !== 'unavailable' &&
          entry.surface.surface_id !== preferredCliSurface?.surface_id,
      },
    ]
  })
  const currentLlmPreferredCli = oauthPanels.find((entry) => entry.endpointKind === 'llm_api')
  const currentOcrUsesNoAuth = surfaceUsesNoAuth(currentOcrSurface, 'ocr_api')
  const currentLlmUsesNoAuth = surfaceUsesNoAuth(currentLlmSurface, 'llm_api')
  const currentOcrSupportsModelSelection = surfaceSupportsModelSelection(currentOcrSurface, 'ocr_api')
  const currentLlmSupportsModelSelection = surfaceSupportsModelSelection(currentLlmSurface, 'llm_api')
  const currentOcrModelSupport = surfaceModelSupportsCapability(
    currentOcrSurface,
    'ocr_api',
    formData.ai_provider.ocr_api?.model,
  )
  const currentOcrStrategy = surfaceOcrExecutionStrategy(currentOcrSurface)
  const currentLlmModelSupport = surfaceModelSupportsCapability(
    currentLlmSurface,
    'llm_api',
    formData.ai_provider.llm_api?.model,
  )

  const usesCustomSelfHostedEndpoint = (
    surface: ProviderSurfaceSpec | undefined,
    endpointKind: EndpointSurfaceKind,
  ): boolean => {
    if (!surface || surface.execution_kind !== 'direct_http' || surface.placement_kind !== 'self_hosted') {
      return false
    }

    const configuredEndpoint =
      endpointKind === 'ocr_api'
        ? formData.ai_provider.ocr_api?.endpoint?.trim()
        : formData.ai_provider.llm_api?.endpoint?.trim()
    const catalogEndpoint = defaultSurfaceEndpoint(surface, endpointKind).trim()

    return Boolean(configuredEndpoint && catalogEndpoint && configuredEndpoint !== catalogEndpoint)
  }

  const surfaceAvailabilityForEndpoint = (
    surface: ProviderSurfaceSpec | undefined,
    endpointKind: EndpointSurfaceKind,
  ) => {
    const endpointProbe = endpointProbeResult[endpointKind]
    if (endpointProbe) {
      return endpointProbe.availability
    }

    return usesCustomSelfHostedEndpoint(surface, endpointKind)
      ? 'partially_available'
      : providerSurfaceAvailability(surface, featureCapabilities)
  }

  const surfaceStatusCopyKeyForEndpoint = (
    surface: ProviderSurfaceSpec | undefined,
    endpointKind: EndpointSurfaceKind,
  ) => {
    const endpointProbe = endpointProbeResult[endpointKind]
    if (endpointProbe) {
      return endpointProbe.status_copy_key
    }

    return usesCustomSelfHostedEndpoint(surface, endpointKind)
      ? null
      : providerSurfaceStatusCopyKey(surface, featureCapabilities)
  }

  const currentLlmAvailability = surfaceAvailabilityForEndpoint(currentLlmSurface, 'llm_api')
  const currentLlmStatusCopyKey = surfaceStatusCopyKeyForEndpoint(currentLlmSurface, 'llm_api')
  const llmProviderLocked = isOAuthAccessMode || isCliAccessMode
  const llmProviderLockReason = isCliAccessMode
    ? t('settingsAutomation.providerSelectionLockedCli')
    : isOAuthAccessMode
      ? t('settingsAutomation.providerSelectionLockedOAuth')
      : null
  const effectiveLlmProvider = isCliAccessMode ? 'Remote' : formData.ai_provider.llm_provider
  const activeOcrPathSummary =
    formData.ai_provider.ocr_provider === 'Remote' && currentOcrSurface
      ? t('settingsAutomation.activePathRemoteSurface', { surface: currentOcrSurface.display_name })
      : formData.ai_provider.ocr_provider === 'Remote'
        ? t('settingsAutomation.activePathRemote')
        : t('settingsAutomation.activePathLocal')
  const activeLlmPathSummary =
    effectiveLlmProvider === 'Remote' && currentLlmSurface
      ? t('settingsAutomation.activePathRemoteSurface', { surface: currentLlmSurface.display_name })
      : effectiveLlmProvider === 'Remote'
        ? t('settingsAutomation.activePathRemote')
        : t('settingsAutomation.activePathLocal')

  const handleSwitchToPreferredCli = (
    endpointKind: EndpointSurfaceKind,
    preferredCliSurface: ProviderSurfaceSpec | undefined,
  ) => {
    settingsForm.handleAiProviderChange('access_mode', 'ProviderSubscriptionCli')
    if (preferredCliSurface) {
      settingsForm.handleProviderSurfaceChange(endpointKind, preferredCliSurface.surface_id)
    }
  }

  const accessModeOptions = [
    {
      value: 'ProviderApiKey',
      label: t('settingsAutomation.accessModeApiKeyLabel'),
      description: t('settingsAutomation.accessModeApiKeyDescription'),
      maturity: 'stable' as const,
      preferred: false,
    },
    {
      value: 'ProviderSubscriptionCli',
      label: t('settingsAutomation.accessModeCliLabel'),
      description: t('settingsAutomation.accessModeCliDescription'),
      maturity: 'beta' as const,
      preferred: true,
    },
    {
      value: 'ProviderOAuth',
      label: t('settingsAutomation.accessModeOAuthLabel'),
      description: t('settingsAutomation.accessModeOAuthDescription'),
      maturity: 'experimental' as const,
      preferred: false,
    },
  ]

  const currentAccessModeOption =
    accessModeOptions.find((option) => option.value === formData.ai_provider.access_mode) ?? accessModeOptions[0]

  const formatSurfaceOptionLabel = (surface: ProviderSurfaceSpec): string => {
    const labels = [surface.display_name, placementKindLabel(t, surface.placement_kind)]
    if (surface.preferred_for_product_auth) {
      labels.push(t('featureCapability.preferredPath'))
    }

    const maturity = providerSurfaceMaturity(surface, featureCapabilities)
    if (maturity !== 'stable') {
      labels.push(t(`featureCapability.maturity.${maturity}`))
    }

    const availability = providerSurfaceAvailability(surface, featureCapabilities)
    if (availability !== 'available') {
      labels.push(t(`featureCapability.availability.${availability}`))
    }

    return labels.join(' · ')
  }

  const renderSurfaceStatus = (surface: ProviderSurfaceSpec | undefined, endpointKind: EndpointSurfaceKind) => {
    if (!surface) {
      return null
    }

    const feature = findFeatureCapability(featureCapabilities, surface.surface_id)
    const maturity = providerSurfaceMaturity(surface, featureCapabilities)
    const customSelfHostedEndpoint = usesCustomSelfHostedEndpoint(surface, endpointKind)
    const endpointProbe = endpointProbeResult[endpointKind]
    const availability = surfaceAvailabilityForEndpoint(surface, endpointKind)
    const statusCopyKey = surfaceStatusCopyKeyForEndpoint(surface, endpointKind)
    const setupCopyKey = endpointProbe ? null : (feature?.setup_copy_key ?? null)
    const setupDocsUrl = endpointProbe ? null : (feature?.setup_docs_url ?? null)
    const setupEnvVars = endpointProbe ? [] : (feature?.configuration_env_vars ?? [])

    return (
      <div className="space-y-2 rounded-lg border border-muted bg-surface-muted/80 p-3">
        <div className="flex flex-wrap items-center gap-2">
          <Badge color="default" size="sm">
            {placementKindLabel(t, surface.placement_kind)}
          </Badge>
          <Badge color={maturityBadgeColor(maturity)} size="sm">
            {t(`featureCapability.maturity.${maturity}`)}
          </Badge>
          {surface.preferred_for_product_auth && (
            <Badge color="info" size="sm">
              {t('featureCapability.preferredPath')}
            </Badge>
          )}
          <span className="text-content-secondary text-xs">{t(`featureCapability.availability.${availability}`)}</span>
        </div>
        {statusCopyKey && <p className="text-content-secondary text-xs">{t(statusCopyKey)}</p>}
        {customSelfHostedEndpoint && !endpointProbe && !endpointProbeLoading[endpointKind] && (
          <p className="text-content-secondary text-xs">{t('settingsAutomation.selfHostedCustomEndpointStatus')}</p>
        )}
        {setupCopyKey && (
          <div className="space-y-2 rounded-md border border-muted bg-surface-elevated/70 p-3">
            <p className="text-content-muted text-xs">{t('featureCapability.setupTitle')}</p>
            <p className="text-content-secondary text-xs">{t(setupCopyKey)}</p>
            {setupEnvVars.length > 0 && (
              <div className="space-y-1">
                <p className="text-content-muted text-xs">{t('featureCapability.setupEnvVars')}</p>
                <div className="flex flex-wrap gap-2">
                  {setupEnvVars.map((envVar) => (
                    <Badge key={envVar} color="default" size="sm">
                      <code>{envVar}</code>
                    </Badge>
                  ))}
                </div>
              </div>
            )}
            {setupDocsUrl && (
              <a
                href={setupDocsUrl}
                target="_blank"
                rel="noreferrer"
                className="inline-flex text-accent text-xs underline"
              >
                {t('featureCapability.openSetupDocs')}
              </a>
            )}
          </div>
        )}
      </div>
    )
  }

  const showDirectApiFields = (surface: ProviderSurfaceSpec | undefined): boolean =>
    (surface?.execution_kind ?? 'direct_http') === 'direct_http'

  const showManagedHttpFields = (surface: ProviderSurfaceSpec | undefined): boolean =>
    surface?.execution_kind === 'managed_http'

  const showSubprocessFields = (surface: ProviderSurfaceSpec | undefined): boolean =>
    surface?.execution_kind === 'subprocess_cli'

  const handleProviderWizardSelect = (provider: ProviderDef, apiKey: string) => {
    settingsForm.handleAiProviderChange('access_mode', 'ProviderApiKey')
    settingsForm.handleAiProviderChange('llm_provider', provider.tier === 'local' ? 'Local' : 'Remote')
    settingsForm.handleProviderSurfaceChange('llm_api', provider.surfaceId)
    if (apiKey) {
      settingsForm.handleExternalApiChange('llm_api', 'api_key_masked', apiKey)
    }
    settingsForm.handleExternalApiChange('llm_api', 'model', provider.defaultModel)
  }

  return (
    <div className="space-y-6">
      <ProviderWizard onSelect={handleProviderWizardSelect} />

      <GuidancePanel
        title={t('settings.guidance.aiAutomation.title')}
        description={t('settings.guidance.aiAutomation.description')}
        items={[
          {
            title: t('settings.guidance.aiAutomation.path.title'),
            description: t('settings.guidance.aiAutomation.path.description'),
          },
          {
            title: t('settings.guidance.aiAutomation.safety.title'),
            description: t('settings.guidance.aiAutomation.safety.description'),
          },
          {
            title: t('settings.guidance.aiAutomation.verify.title'),
            description: t('settings.guidance.aiAutomation.verify.description'),
          },
        ]}
        footer={t('settings.guidance.aiAutomation.footer')}
      />

      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settingsAutomation.title')}</CardTitle>
        <div className="space-y-4">
          <ToggleRow
            label={t('settingsAutomation.enabled')}
            description={t('settingsAutomation.enabledDescription')}
            checked={formData.automation.enabled}
            onChange={(value) => settingsForm.handleAutomationChange('enabled', value)}
          />
        </div>
      </Card>

      <SandboxConfig formData={formData} onSandboxChange={settingsForm.handleSandboxChange} />

      <Card id="section-ai" variant="default" padding="lg">
        <CardTitle sticky>{t('settingsAutomation.aiTitle')}</CardTitle>
        <div className="space-y-4">
          <div className="space-y-3 rounded-lg border border-muted p-4">
            <div>
              <label htmlFor="settings-ai-access-mode" className={form.label}>
                {t('settingsAutomation.accessModeLabel')}
              </label>
              <Select
                id="settings-ai-access-mode"
                value={formData.ai_provider.access_mode}
                onChange={(e) => settingsForm.handleAiProviderChange('access_mode', e.target.value)}
              >
                {accessModeOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
                {!accessModeOptions.some((option) => option.value === formData.ai_provider.access_mode) && (
                  <option value={formData.ai_provider.access_mode}>{currentAccessModeOption.label}</option>
                )}
              </Select>
              <FieldHint>{t('settingsAutomation.accessModeHint')}</FieldHint>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Badge color={maturityBadgeColor(currentAccessModeOption.maturity)} size="sm">
                {t(`featureCapability.maturity.${currentAccessModeOption.maturity}`)}
              </Badge>
              {currentAccessModeOption.preferred && (
                <Badge color="info" size="sm">
                  {t('featureCapability.preferredPath')}
                </Badge>
              )}
            </div>
            <p className="text-content-secondary text-sm">{currentAccessModeOption.description}</p>
          </div>

          <ProfileManager
            formData={formData}
            onSelectAiProviderProfile={settingsForm.handleSelectAiProviderProfile}
            onSaveAiProviderProfile={settingsForm.handleSaveAiProviderProfile}
            onDeleteAiProviderProfile={settingsForm.handleDeleteAiProviderProfile}
          />

          {showOcrRemoteSection && currentOcrSurface && (
            <div className="space-y-3 rounded-lg border border-muted bg-surface-muted/80 p-4">
              <div className="space-y-1">
                <p className={`${typography.weight.medium} text-content-strong text-sm`}>
                  OCR {t('settingsAutomation.providerSurfaceLabel')}
                </p>
                <p className="text-content-secondary text-sm">
                  {t('settingsAutomation.pathSummarySurface', { surface: currentOcrSurface.display_name })}
                </p>
              </div>

              <div>
                <label htmlFor="settings-ocr-provider-surface" className="mb-1 block text-content-secondary text-xs">
                  {t('settingsAutomation.providerSurfaceLabel')}
                </label>
                <Select
                  id="settings-ocr-provider-surface"
                  value={
                    formData.ai_provider.ocr_api?.surface_id ??
                    settingsForm.resolveEndpointSurface('ocr_api')?.surface_id ??
                    ''
                  }
                  disabled={providerSurfaceOptions.ocr_api.length === 0}
                  onChange={(e) => settingsForm.handleProviderSurfaceChange('ocr_api', e.target.value)}
                >
                  {providerSurfaceOptions.ocr_api.length === 0 ? (
                    <option value="">{t('settingsAutomation.noCompatibleProviderSurface')}</option>
                  ) : (
                    providerSurfaceOptions.ocr_api.map((surface) => (
                      <option key={surface.surface_id} value={surface.surface_id}>
                        {formatSurfaceOptionLabel(surface)}
                      </option>
                    ))
                  )}
                </Select>
                <FieldHint>{t('settingsAutomation.providerSurfaceHint')}</FieldHint>
              </div>

              {renderSurfaceStatus(currentOcrSurface, 'ocr_api')}

              {currentOcrStrategy !== 'none' && (
                <div className="rounded-md bg-surface-elevated/70 p-3 text-content-secondary text-xs">
                  {currentOcrStrategy === 'multimodal_llm'
                    ? t('settingsAutomation.ocrStrategyMultimodal')
                    : t('settingsAutomation.ocrStrategyVisionApi')}
                </div>
              )}
            </div>
          )}

          {showLlmSurfaceSection && currentLlmSurface && (
            <div className="space-y-3 rounded-lg border border-muted bg-surface-muted/80 p-4">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div className="space-y-1">
                  <p className={`${typography.weight.medium} text-content-strong text-sm`}>
                    LLM {t('settingsAutomation.providerSurfaceLabel')}
                  </p>
                  <p className="text-content-secondary text-sm">
                    {t('settingsAutomation.pathSummarySurface', { surface: currentLlmSurface.display_name })}
                  </p>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <Badge color={maturityBadgeColor(currentLlmMaturity)} size="sm">
                    {t(`featureCapability.maturity.${currentLlmMaturity}`)}
                  </Badge>
                  {currentLlmSurface.preferred_for_product_auth && (
                    <Badge color="info" size="sm">
                      {t('featureCapability.preferredPath')}
                    </Badge>
                  )}
                  <Badge color={currentLlmAvailability === 'available' ? 'success' : 'warning'} size="sm">
                    {t(`featureCapability.availability.${currentLlmAvailability}`)}
                  </Badge>
                </div>
              </div>

              <div>
                <label htmlFor="settings-llm-provider-surface" className="mb-1 block text-content-secondary text-xs">
                  {t('settingsAutomation.providerSurfaceLabel')}
                </label>
                <Select
                  id="settings-llm-provider-surface"
                  value={
                    formData.ai_provider.llm_api?.surface_id ??
                    settingsForm.resolveEndpointSurface('llm_api')?.surface_id ??
                    ''
                  }
                  disabled={providerSurfaceOptions.llm_api.length === 0}
                  onChange={(e) => settingsForm.handleProviderSurfaceChange('llm_api', e.target.value)}
                >
                  {providerSurfaceOptions.llm_api.length === 0 ? (
                    <option value="">{t('settingsAutomation.noCompatibleProviderSurface')}</option>
                  ) : (
                    providerSurfaceOptions.llm_api.map((surface) => (
                      <option key={surface.surface_id} value={surface.surface_id}>
                        {formatSurfaceOptionLabel(surface)}
                      </option>
                    ))
                  )}
                </Select>
                <FieldHint>{t('settingsAutomation.providerSurfaceHint')}</FieldHint>
              </div>

              <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
                <div className="space-y-1">
                  <p className="text-content-muted text-xs">{t('settingsAutomation.pathExecutionLabel')}</p>
                  <p className="text-content-secondary text-sm">
                    {executionKindLabel(t, currentLlmSurface.execution_kind)}
                  </p>
                </div>
                <div className="space-y-1">
                  <p className="text-content-muted text-xs">{t('settingsAutomation.pathPlacementLabel')}</p>
                  <p className="text-content-secondary text-sm">
                    {placementKindLabel(t, currentLlmSurface.placement_kind)}
                  </p>
                  <p className="text-content-muted text-xs">
                    {placementKindDescription(t, currentLlmSurface.placement_kind)}
                  </p>
                </div>
                <div className="space-y-1">
                  <p className="text-content-muted text-xs">{t('settingsAutomation.pathRequirementsLabel')}</p>
                  <div className="flex flex-wrap gap-2">
                    {currentLlmRequirements.length > 0 ? (
                      currentLlmRequirements.map((requirement) => (
                        <Badge key={requirement} color="default" size="sm">
                          {requirementLabel(t, requirement)}
                        </Badge>
                      ))
                    ) : (
                      <span className="text-content-secondary text-sm">
                        {t('settingsAutomation.pathRequirementsNone')}
                      </span>
                    )}
                  </div>
                </div>
              </div>

              {currentLlmStatusCopyKey && (
                <div className="space-y-1">
                  <p className="text-content-muted text-xs">{t('settingsAutomation.pathNextStepLabel')}</p>
                  <p className="text-content-secondary text-sm">{t(currentLlmStatusCopyKey)}</p>
                </div>
              )}

              {currentLlmPreferredCli?.showPreferredCliCta && currentLlmPreferredCli.preferredCliSurface && (
                <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-muted bg-surface-elevated/70 p-3">
                  <div className="space-y-1">
                    <p className={`${typography.weight.medium} text-content-strong text-sm`}>
                      {t('settingsAutomation.preferredCliTitle')}
                    </p>
                    <p className="text-content-secondary text-sm">
                      {t('settingsAutomation.preferredCliDescription', {
                        surface: currentLlmPreferredCli.preferredCliSurface.display_name,
                      })}
                    </p>
                  </div>
                  <Button
                    type="button"
                    variant="secondary"
                    size="sm"
                    onClick={() =>
                      handleSwitchToPreferredCli(
                        currentLlmPreferredCli.endpointKind,
                        currentLlmPreferredCli.preferredCliSurface,
                      )
                    }
                  >
                    {t('settingsAutomation.switchToPreferredCli')}
                  </Button>
                </div>
              )}
            </div>
          )}

          <div className="space-y-3 rounded-lg border border-muted bg-surface-muted/80 p-4">
            <div className="space-y-1">
              <p className={`${typography.weight.medium} text-content-strong text-sm`}>
                {t('settingsAutomation.activeRoutingTitle')}
              </p>
              <p className="text-content-secondary text-sm">{t('settingsAutomation.activeRoutingDescription')}</p>
            </div>
            <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
              <div className="space-y-2 rounded-md border border-muted/70 bg-surface-elevated/70 p-3">
                <div className="flex items-center justify-between gap-2">
                  <span className={`${typography.weight.medium} text-content text-sm`}>
                    {t('settingsAutomation.ocrProvider')}
                  </span>
                  <Badge color={formData.ai_provider.ocr_provider === 'Remote' ? 'info' : 'default'} size="sm">
                    {formData.ai_provider.ocr_provider === 'Remote'
                      ? t('settingsAutomation.providerRemote')
                      : t('settingsAutomation.providerLocal')}
                  </Badge>
                </div>
                <p className="text-content-secondary text-sm">{activeOcrPathSummary}</p>
              </div>
              <div className="space-y-2 rounded-md border border-muted/70 bg-surface-elevated/70 p-3">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2">
                    <span className={`${typography.weight.medium} text-content text-sm`}>
                      {t('settingsAutomation.llmProvider')}
                    </span>
                    {llmProviderLocked && (
                      <Badge color="warning" size="sm">
                        {t('settingsAutomation.providerSelectionLockedBadge')}
                      </Badge>
                    )}
                  </div>
                  <Badge color={effectiveLlmProvider === 'Remote' ? 'info' : 'default'} size="sm">
                    {effectiveLlmProvider === 'Remote'
                      ? t('settingsAutomation.providerRemote')
                      : t('settingsAutomation.providerLocal')}
                  </Badge>
                </div>
                <p className="text-content-secondary text-sm">{activeLlmPathSummary}</p>
                {llmProviderLocked && llmProviderLockReason && (
                  <p className="text-content-muted text-xs">{llmProviderLockReason}</p>
                )}
              </div>
            </div>
          </div>

          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <div>
              <label htmlFor="settings-ocr-provider" className={form.label}>
                {t('settingsAutomation.ocrProvider')}
              </label>
              <Select
                id="settings-ocr-provider"
                value={formData.ai_provider.ocr_provider}
                onChange={(e) => settingsForm.handleAiProviderChange('ocr_provider', e.target.value)}
              >
                <option value="Local">{t('settingsAutomation.providerLocal')}</option>
                <option value="Remote">{t('settingsAutomation.providerRemote')}</option>
              </Select>
              <FieldHint>{t('settingsAutomation.ocrProviderHint')}</FieldHint>
            </div>
            <div>
              <label htmlFor="settings-llm-provider" className={form.label}>
                {t('settingsAutomation.llmProvider')}
              </label>
              {llmProviderLocked && (
                <div className="mb-2 flex items-center gap-2">
                  <Badge color="warning" size="sm">
                    {t('settingsAutomation.providerSelectionLockedBadge')}
                  </Badge>
                  <span className="text-content-secondary text-xs">{llmProviderLockReason}</span>
                </div>
              )}
              <Select
                id="settings-llm-provider"
                value={isCliAccessMode ? 'Remote' : formData.ai_provider.llm_provider}
                disabled={llmProviderLocked}
                onChange={(e) => settingsForm.handleAiProviderChange('llm_provider', e.target.value)}
              >
                <option value="Local">{t('settingsAutomation.providerLocal')}</option>
                <option value="Remote">{t('settingsAutomation.providerRemote')}</option>
              </Select>
              <FieldHint>{t('settingsAutomation.llmProviderHint')}</FieldHint>
              {llmProviderLocked && (
                <p className="mt-1 text-content-secondary text-xs">
                  {isCliAccessMode
                    ? t('settingsAutomation.cliModeProviderSummary')
                    : t('settingsAutomation.oauthLlmProviderPinned')}
                </p>
              )}
            </div>
          </div>

          <div>
            <label htmlFor="settings-data-policy" className={form.label}>
              {t('settingsAutomation.dataPolicy')}
            </label>
            <Select
              id="settings-data-policy"
              value={formData.ai_provider.external_data_policy}
              onChange={(e) => settingsForm.handleAiProviderChange('external_data_policy', e.target.value)}
            >
              <option value="PiiFilterStrict">{t('settingsAutomation.dataPolicyStrict')}</option>
              <option value="PiiFilterStandard">{t('settingsAutomation.dataPolicyStandard')}</option>
              <option value="AllowFiltered">{t('settingsAutomation.dataPolicyAllowFiltered')}</option>
            </Select>
            <FieldHint>{t('settingsAutomation.dataPolicyHint')}</FieldHint>
          </div>

          <ToggleRow
            label={t('settingsAutomation.allowUnredactedExternalOcr')}
            description={t('settingsAutomation.allowUnredactedExternalOcrDescription')}
            checked={formData.ai_provider.allow_unredacted_external_ocr}
            onChange={(value) => settingsForm.handleAiProviderChange('allow_unredacted_external_ocr', value)}
          />

          <SceneIntelligenceConfig
            formData={formData}
            onSceneActionOverrideChange={settingsForm.handleSceneActionOverrideChange}
            onSceneIntelligenceChange={settingsForm.handleSceneIntelligenceChange}
            onOcrValidationChange={settingsForm.handleOcrValidationChange}
          />

          <ToggleRow
            label={t('settingsAutomation.fallbackToLocal')}
            description={t('settingsAutomation.fallbackToLocalDescription')}
            checked={formData.ai_provider.fallback_to_local}
            onChange={(value) => settingsForm.handleAiProviderChange('fallback_to_local', value)}
          />

          {isOAuthAccessMode &&
            oauthPanels.map((panel) => (
              <OAuthConnectionPanel
                key={`${panel.endpointKind}:${panel.oauthSurface.surface_id}`}
                providerId={panel.oauthSurface.vendor_id}
                providerName={
                  panel.oauthSurface.provider_type === 'OpenAi' ? 'OpenAI' : panel.oauthSurface.display_name
                }
                oauthSurface={panel.oauthSurface}
                preferredCliSurface={panel.preferredCliSurface}
                featureSnapshot={featureCapabilities}
                secretBackendCapabilities={secretBackendCapabilities}
                onUsePreferredCli={
                  panel.showPreferredCliCta
                    ? () => handleSwitchToPreferredCli(panel.endpointKind, panel.preferredCliSurface)
                    : undefined
                }
              />
            ))}

          {showOcrRemoteSection && (
            <OcrEndpointConfig
              formData={formData}
              endpointKind="ocr_api"
              surface={currentOcrSurface}
              usesNoAuth={currentOcrUsesNoAuth}
              supportsModelSelection={currentOcrSupportsModelSelection}
              modelSupport={currentOcrModelSupport}
              modelCatalogNotice={modelCatalogNotice.ocr_api}
              modelCompatibilityNotice={modelCompatibilityNotice.ocr_api}
              modelCatalogLoading={modelCatalogLoading}
              endpointProbeResult={endpointProbeResult.ocr_api}
              endpointProbeLoading={endpointProbeLoading.ocr_api}
              onExternalApiChange={settingsForm.handleExternalApiChange}
              onDiscoverModels={settingsForm.discoverModels}
              getModelOptions={settingsForm.getModelOptions}
              canDiscoverModels={settingsForm.canDiscoverModels}
              showDirectApiFields={showDirectApiFields(currentOcrSurface)}
              showManagedHttpFields={showManagedHttpFields(currentOcrSurface)}
              showSubprocessFields={showSubprocessFields(currentOcrSurface)}
            />
          )}

          {showLlmSurfaceSection && (
            <LlmEndpointConfig
              formData={formData}
              endpointKind="llm_api"
              surface={currentLlmSurface}
              usesNoAuth={currentLlmUsesNoAuth}
              supportsModelSelection={currentLlmSupportsModelSelection}
              modelSupport={currentLlmModelSupport}
              modelCatalogNotice={modelCatalogNotice.llm_api}
              modelCompatibilityNotice={modelCompatibilityNotice.llm_api}
              modelCatalogLoading={modelCatalogLoading}
              endpointProbeResult={endpointProbeResult.llm_api}
              endpointProbeLoading={endpointProbeLoading.llm_api}
              onExternalApiChange={settingsForm.handleExternalApiChange}
              onDiscoverModels={settingsForm.discoverModels}
              getModelOptions={settingsForm.getModelOptions}
              canDiscoverModels={settingsForm.canDiscoverModels}
              showDirectApiFields={showDirectApiFields(currentLlmSurface)}
              showManagedHttpFields={showManagedHttpFields(currentLlmSurface)}
              showSubprocessFields={showSubprocessFields(currentLlmSurface)}
            />
          )}
        </div>
      </Card>
    </div>
  )
}
