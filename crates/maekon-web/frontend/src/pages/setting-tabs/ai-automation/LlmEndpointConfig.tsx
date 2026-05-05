import { useTranslation } from 'react-i18next'
import { Button, FieldHint, Input } from '../../../components/ui'
import { typography } from '../../../styles/tokens'
import {
  apiKeyPlaceholder,
  credentialBackendLabel,
  placementKindDescription,
  shouldShowBackendManagedHint,
  supportsProjectionToggle,
} from '../ai-automation-utils'
import ToggleRow from '../ToggleRow'
import type { EndpointConfigProps } from './types'

export default function LlmEndpointConfig({
  formData,
  surface,
  usesNoAuth,
  supportsModelSelection,
  modelSupport,
  modelCatalogNotice,
  modelCompatibilityNotice,
  modelCatalogLoading,
  onExternalApiChange,
  onDiscoverModels,
  getModelOptions,
  canDiscoverModels,
  showDirectApiFields,
  showManagedHttpFields,
  showSubprocessFields,
}: EndpointConfigProps) {
  const { t } = useTranslation()

  return (
    <div className="space-y-3 rounded-lg border border-muted p-4">
      <h4 className={`${typography.weight.medium} text-content-strong text-sm`}>
        LLM {t('settingsAutomation.externalApi')}
      </h4>
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        {showDirectApiFields && (
          <>
            <div className="flex items-end">
              <Button
                type="button"
                variant="secondary"
                size="sm"
                disabled={!canDiscoverModels('llm_api')}
                isLoading={modelCatalogLoading === 'llm_api'}
                onClick={() => onDiscoverModels('llm_api')}
              >
                {t('settingsAutomation.loadModels')}
              </Button>
            </div>
            <div>
              <label htmlFor="settings-llm-endpoint" className="mb-1 block text-content-secondary text-xs">
                {t('settingsAutomation.endpoint')}
              </label>
              <Input
                id="settings-llm-endpoint"
                type="text"
                value={formData.ai_provider.llm_api?.endpoint ?? ''}
                onChange={(e) => onExternalApiChange('llm_api', 'endpoint', e.target.value)}
                placeholder={t('settingsAutomation.endpointPlaceholderLlm')}
              />
              <FieldHint>{t('settingsAutomation.llmEndpointHint')}</FieldHint>
            </div>
            {!usesNoAuth ? (
              <div>
                <label htmlFor="settings-llm-api-key" className="mb-1 block text-content-secondary text-xs">
                  {t('settingsAutomation.apiKey')}
                </label>
                <Input
                  id="settings-llm-api-key"
                  type="password"
                  value={formData.ai_provider.llm_api?.api_key_masked ?? ''}
                  onChange={(e) => onExternalApiChange('llm_api', 'api_key_masked', e.target.value)}
                  placeholder={apiKeyPlaceholder(t, formData.ai_provider.llm_api)}
                />
                <FieldHint>{t('settingsAutomation.apiKeyHint')}</FieldHint>
                {shouldShowBackendManagedHint(formData.ai_provider.llm_api) && (
                  <p className="mt-1 text-content-secondary text-xs">
                    {t('settingsAutomation.apiKeyStoredHint', {
                      backend: credentialBackendLabel(t, formData.ai_provider.llm_api?.backend_kind),
                    })}
                  </p>
                )}
              </div>
            ) : (
              <div className="rounded-md bg-surface-muted/80 p-3 text-content-secondary text-xs md:col-span-2">
                {placementKindDescription(t, surface?.placement_kind)}
              </div>
            )}
            {usesNoAuth && (
              <div className="rounded-md bg-surface-elevated/70 p-3 text-content-secondary text-xs md:col-span-2">
                {t('settingsAutomation.noAuthSurfaceDescription')}
              </div>
            )}
            {!usesNoAuth && supportsProjectionToggle(formData.ai_provider.llm_api) && (
              <div className="md:col-span-2">
                <ToggleRow
                  label={t('settingsAutomation.secretProjectionEnabled')}
                  description={t('settingsAutomation.secretProjectionEnabledDescription')}
                  checked={formData.ai_provider.llm_api?.projection_enabled ?? false}
                  onChange={(value) => onExternalApiChange('llm_api', 'projection_enabled', value)}
                />
              </div>
            )}
          </>
        )}
        {showManagedHttpFields && (
          <div className="rounded-lg border border-muted bg-surface-muted/80 p-3 md:col-span-2">
            <p className="text-content-secondary text-sm">{t('settingsAutomation.managedOAuthSurfaceDescription')}</p>
          </div>
        )}
        {showSubprocessFields && (
          <div className="rounded-lg border border-muted bg-surface-muted/80 p-3 md:col-span-2">
            <p className="text-content-secondary text-sm">{t('settingsAutomation.subprocessSurfaceDescription')}</p>
            {surface?.subprocess_transport?.executable_candidates?.length ? (
              <p className="mt-2 text-content-muted text-xs">
                {t('settingsAutomation.subprocessExecutableHint', {
                  executables: surface.subprocess_transport.executable_candidates.join(', '),
                })}
              </p>
            ) : null}
          </div>
        )}
        {supportsModelSelection ? (
          <div>
            <label htmlFor="settings-llm-model" className="mb-1 block text-content-secondary text-xs">
              {t('settingsAutomation.model')}
            </label>
            <Input
              id="settings-llm-model"
              type="text"
              list="llm-model-catalog"
              value={formData.ai_provider.llm_api?.model ?? ''}
              onChange={(e) => onExternalApiChange('llm_api', 'model', e.target.value || null)}
            />
            <FieldHint>{t('settingsAutomation.llmModelHint')}</FieldHint>
            {getModelOptions('llm_api').length > 0 && (
              <datalist id="llm-model-catalog">
                {getModelOptions('llm_api').map((modelName) => (
                  <option key={modelName} value={modelName} />
                ))}
              </datalist>
            )}
            {(modelCompatibilityNotice || modelSupport === false) && (
              <p className="mt-1 text-semantic-warning text-xs">
                {modelCompatibilityNotice ?? t('settingsAutomation.llmModelUnsupported')}
              </p>
            )}
          </div>
        ) : (
          <div className="rounded-md bg-surface-muted/80 p-3 text-content-secondary text-xs">
            {t('settingsAutomation.modelSelectionUnsupportedSurface')}
          </div>
        )}
        <div>
          <label htmlFor="settings-llm-timeout" className="mb-1 block text-content-secondary text-xs">
            {t('settingsAutomation.timeoutSecs')}
          </label>
          <Input
            id="settings-llm-timeout"
            type="number"
            min={5}
            max={300}
            value={formData.ai_provider.llm_api?.timeout_secs ?? 30}
            onChange={(e) => onExternalApiChange('llm_api', 'timeout_secs', parseInt(e.target.value, 10) || 30)}
          />
          <FieldHint>{t('settingsAutomation.timeoutHint')}</FieldHint>
        </div>
      </div>
      {showDirectApiFields && modelCatalogNotice && (
        <p className="text-content-secondary text-xs">{modelCatalogNotice}</p>
      )}
      {showDirectApiFields && !canDiscoverModels('llm_api') && !modelCatalogNotice && (
        <p className="text-content-secondary text-xs">{t('settingsAutomation.modelDiscoveryUnsupportedSurface')}</p>
      )}
    </div>
  )
}
