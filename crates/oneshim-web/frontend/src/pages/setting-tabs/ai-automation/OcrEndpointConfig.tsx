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

export default function OcrEndpointConfig({
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
}: EndpointConfigProps) {
  const { t } = useTranslation()

  return (
    <div className="space-y-3 rounded-lg border border-muted p-4">
      <h4 className={`${typography.weight.medium} text-content-strong text-sm`}>
        OCR {t('settingsAutomation.externalApi')}
      </h4>
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        <div className="flex items-end">
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={!canDiscoverModels('ocr_api')}
            isLoading={modelCatalogLoading === 'ocr_api'}
            onClick={() => onDiscoverModels('ocr_api')}
          >
            {t('settingsAutomation.loadModels')}
          </Button>
        </div>
        {showDirectApiFields && (
          <>
            <div>
              <label htmlFor="settings-ocr-endpoint" className="mb-1 block text-content-secondary text-xs">
                {t('settingsAutomation.endpoint')}
              </label>
              <Input
                id="settings-ocr-endpoint"
                type="text"
                value={formData.ai_provider.ocr_api?.endpoint ?? ''}
                onChange={(e) => onExternalApiChange('ocr_api', 'endpoint', e.target.value)}
                placeholder={t('settingsAutomation.endpointPlaceholderOcr')}
              />
              <FieldHint>{t('settingsAutomation.ocrEndpointHint')}</FieldHint>
            </div>
            {!usesNoAuth ? (
              <div>
                <label htmlFor="settings-ocr-api-key" className="mb-1 block text-content-secondary text-xs">
                  {t('settingsAutomation.apiKey')}
                </label>
                <Input
                  id="settings-ocr-api-key"
                  type="password"
                  value={formData.ai_provider.ocr_api?.api_key_masked ?? ''}
                  onChange={(e) => onExternalApiChange('ocr_api', 'api_key_masked', e.target.value)}
                  placeholder={apiKeyPlaceholder(t, formData.ai_provider.ocr_api)}
                />
                <FieldHint>{t('settingsAutomation.apiKeyHint')}</FieldHint>
                {shouldShowBackendManagedHint(formData.ai_provider.ocr_api) && (
                  <p className="mt-1 text-content-secondary text-xs">
                    {t('settingsAutomation.apiKeyStoredHint', {
                      backend: credentialBackendLabel(t, formData.ai_provider.ocr_api?.backend_kind),
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
            {!usesNoAuth && supportsProjectionToggle(formData.ai_provider.ocr_api) && (
              <div className="md:col-span-2">
                <ToggleRow
                  label={t('settingsAutomation.secretProjectionEnabled')}
                  description={t('settingsAutomation.secretProjectionEnabledDescription')}
                  checked={formData.ai_provider.ocr_api?.projection_enabled ?? false}
                  onChange={(value) => onExternalApiChange('ocr_api', 'projection_enabled', value)}
                />
              </div>
            )}
          </>
        )}
        {supportsModelSelection ? (
          <div>
            <label htmlFor="settings-ocr-model" className="mb-1 block text-content-secondary text-xs">
              {t('settingsAutomation.model')}
            </label>
            <Input
              id="settings-ocr-model"
              type="text"
              list="ocr-model-catalog"
              value={formData.ai_provider.ocr_api?.model ?? ''}
              onChange={(e) => onExternalApiChange('ocr_api', 'model', e.target.value || null)}
            />
            <FieldHint>{t('settingsAutomation.ocrModelHint')}</FieldHint>
            {getModelOptions('ocr_api').length > 0 && (
              <datalist id="ocr-model-catalog">
                {getModelOptions('ocr_api').map((modelName) => (
                  <option key={modelName} value={modelName} />
                ))}
              </datalist>
            )}
            {(modelCompatibilityNotice || modelSupport === false) && (
              <p className="mt-1 text-semantic-warning text-xs">
                {modelCompatibilityNotice ?? t('settingsAutomation.ocrModelUnsupported')}
              </p>
            )}
          </div>
        ) : (
          <div className="rounded-md bg-surface-muted/80 p-3 text-content-secondary text-xs">
            {t('settingsAutomation.modelSelectionUnsupportedSurface')}
          </div>
        )}
        <div>
          <label htmlFor="settings-ocr-timeout" className="mb-1 block text-content-secondary text-xs">
            {t('settingsAutomation.timeoutSecs')}
          </label>
          <Input
            id="settings-ocr-timeout"
            type="number"
            min={5}
            max={300}
            value={formData.ai_provider.ocr_api?.timeout_secs ?? 30}
            onChange={(e) => onExternalApiChange('ocr_api', 'timeout_secs', parseInt(e.target.value, 10) || 30)}
          />
          <FieldHint>{t('settingsAutomation.timeoutHint')}</FieldHint>
        </div>
      </div>
      {modelCatalogNotice && <p className="text-content-secondary text-xs">{modelCatalogNotice}</p>}
      {!canDiscoverModels('ocr_api') && !modelCatalogNotice && (
        <p className="text-content-secondary text-xs">{t('settingsAutomation.modelDiscoveryUnsupportedSurface')}</p>
      )}
    </div>
  )
}
