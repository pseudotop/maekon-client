import { useTranslation } from 'react-i18next'
import { FieldHint, Input } from '../../../components/ui'
import { typography } from '../../../styles/tokens'
import { toDateTimeLocalValue, toRfc3339OrNull } from '../ai-automation-utils'
import ToggleRow from '../ToggleRow'
import type { SceneIntelligenceConfigProps } from './types'

export default function SceneIntelligenceConfig({
  formData,
  onSceneActionOverrideChange,
  onSceneIntelligenceChange,
  onOcrValidationChange,
}: SceneIntelligenceConfigProps) {
  const { t } = useTranslation()

  return (
    <>
      <div className="space-y-3 rounded-lg border border-muted p-4">
        <h4 className={`${typography.weight.medium} text-content-strong text-sm`}>
          {t('settingsAutomation.sceneActionOverrideTitle')}
        </h4>
        <ToggleRow
          label={t('settingsAutomation.sceneActionOverrideEnabled')}
          description={t('settingsAutomation.sceneActionOverrideEnabledDescription')}
          checked={formData.ai_provider.scene_action_override.enabled}
          onChange={(value) => onSceneActionOverrideChange('enabled', value)}
        />
        <div
          className={`grid grid-cols-1 gap-3 md:grid-cols-2 ${!formData.ai_provider.scene_action_override.enabled ? 'pointer-events-none opacity-50' : ''}`}
        >
          <div className="md:col-span-2">
            <label htmlFor="settings-scene-override-reason" className="mb-1 block text-content-secondary text-xs">
              {t('settingsAutomation.sceneActionOverrideReason')}
            </label>
            <Input
              id="settings-scene-override-reason"
              type="text"
              value={formData.ai_provider.scene_action_override.reason}
              onChange={(e) => onSceneActionOverrideChange('reason', e.target.value)}
              placeholder={t('settingsAutomation.sceneActionOverrideReasonPlaceholder')}
            />
            <FieldHint>{t('settingsAutomation.sceneActionOverrideReasonHint')}</FieldHint>
          </div>
          <div>
            <label htmlFor="settings-scene-override-approved-by" className="mb-1 block text-content-secondary text-xs">
              {t('settingsAutomation.sceneActionOverrideApprovedBy')}
            </label>
            <Input
              id="settings-scene-override-approved-by"
              type="text"
              value={formData.ai_provider.scene_action_override.approved_by}
              onChange={(e) => onSceneActionOverrideChange('approved_by', e.target.value)}
              placeholder={t('settingsAutomation.sceneActionOverrideApprovedByPlaceholder')}
            />
          </div>
          <div>
            <label htmlFor="settings-scene-override-expires" className="mb-1 block text-content-secondary text-xs">
              {t('settingsAutomation.sceneActionOverrideExpiresAt')}
            </label>
            <Input
              id="settings-scene-override-expires"
              type="datetime-local"
              value={toDateTimeLocalValue(formData.ai_provider.scene_action_override.expires_at)}
              onChange={(e) => onSceneActionOverrideChange('expires_at', toRfc3339OrNull(e.target.value))}
            />
            <FieldHint>{t('settingsAutomation.sceneActionOverrideExpiryHint')}</FieldHint>
          </div>
        </div>
      </div>

      <div className="space-y-3 rounded-lg border border-muted p-4">
        <h4 className={`${typography.weight.medium} text-content-strong text-sm`}>
          {t('settingsAutomation.sceneIntelligenceTitle')}
        </h4>
        <ToggleRow
          label={t('settingsAutomation.sceneIntelligenceEnabled')}
          description={t('settingsAutomation.sceneIntelligenceEnabledDescription')}
          checked={formData.ai_provider.scene_intelligence.enabled}
          onChange={(value) => onSceneIntelligenceChange('enabled', value)}
        />
        <div
          className={`space-y-3 ${!formData.ai_provider.scene_intelligence.enabled ? 'pointer-events-none opacity-50' : ''}`}
        >
          <ToggleRow
            label={t('settingsAutomation.sceneOverlayEnabled')}
            description={t('settingsAutomation.sceneOverlayEnabledDescription')}
            checked={formData.ai_provider.scene_intelligence.overlay_enabled}
            onChange={(value) => onSceneIntelligenceChange('overlay_enabled', value)}
          />
          <ToggleRow
            label={t('settingsAutomation.sceneAllowExecution')}
            description={t('settingsAutomation.sceneAllowExecutionDescription')}
            checked={formData.ai_provider.scene_intelligence.allow_action_execution}
            onChange={(value) => onSceneIntelligenceChange('allow_action_execution', value)}
          />
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
            <div>
              <label htmlFor="settings-scene-min-confidence" className="mb-1 block text-content-secondary text-xs">
                {t('settingsAutomation.sceneMinConfidence')}
              </label>
              <Input
                id="settings-scene-min-confidence"
                type="number"
                min={0}
                max={1}
                step={0.05}
                value={formData.ai_provider.scene_intelligence.min_confidence}
                onChange={(e) => onSceneIntelligenceChange('min_confidence', Number(e.target.value))}
              />
              <FieldHint>{t('settingsAutomation.sceneMinConfidenceHint')}</FieldHint>
            </div>
            <div>
              <label htmlFor="settings-scene-max-elements" className="mb-1 block text-content-secondary text-xs">
                {t('settingsAutomation.sceneMaxElements')}
              </label>
              <Input
                id="settings-scene-max-elements"
                type="number"
                min={1}
                max={1000}
                step={1}
                value={formData.ai_provider.scene_intelligence.max_elements}
                onChange={(e) => onSceneIntelligenceChange('max_elements', Number(e.target.value))}
              />
              <FieldHint>{t('settingsAutomation.sceneMaxElementsHint')}</FieldHint>
            </div>
          </div>
          <div className="space-y-3 rounded-md bg-surface-elevated/70 p-3">
            <ToggleRow
              label={t('settingsAutomation.sceneCalibrationEnabled')}
              description={t('settingsAutomation.sceneCalibrationEnabledDescription')}
              checked={formData.ai_provider.scene_intelligence.calibration_enabled}
              onChange={(value) => onSceneIntelligenceChange('calibration_enabled', value)}
            />
            <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
              <div>
                <label htmlFor="settings-scene-cal-min-elements" className="mb-1 block text-content-secondary text-xs">
                  {t('settingsAutomation.sceneCalibrationMinElements')}
                </label>
                <Input
                  id="settings-scene-cal-min-elements"
                  type="number"
                  min={1}
                  max={1000}
                  step={1}
                  value={formData.ai_provider.scene_intelligence.calibration_min_elements}
                  onChange={(e) => onSceneIntelligenceChange('calibration_min_elements', Number(e.target.value))}
                />
                <FieldHint>{t('settingsAutomation.sceneCalibrationMinElementsHint')}</FieldHint>
              </div>
              <div>
                <label
                  htmlFor="settings-scene-cal-min-confidence"
                  className="mb-1 block text-content-secondary text-xs"
                >
                  {t('settingsAutomation.sceneCalibrationMinAvgConfidence')}
                </label>
                <Input
                  id="settings-scene-cal-min-confidence"
                  type="number"
                  min={0}
                  max={1}
                  step={0.05}
                  value={formData.ai_provider.scene_intelligence.calibration_min_avg_confidence}
                  onChange={(e) => onSceneIntelligenceChange('calibration_min_avg_confidence', Number(e.target.value))}
                />
                <FieldHint>{t('settingsAutomation.sceneCalibrationMinAvgConfidenceHint')}</FieldHint>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div className="space-y-3 rounded-lg border border-muted p-4">
        <h4 className={`${typography.weight.medium} text-content-strong text-sm`}>
          {t('settingsAutomation.ocrValidationTitle')}
        </h4>
        <ToggleRow
          label={t('settingsAutomation.ocrValidationEnabled')}
          description={t('settingsAutomation.ocrValidationEnabledDescription')}
          checked={formData.ai_provider.ocr_validation.enabled}
          onChange={(value) => onOcrValidationChange('enabled', value)}
        />
        <div
          className={`grid grid-cols-1 gap-3 md:grid-cols-2 ${!formData.ai_provider.ocr_validation.enabled ? 'pointer-events-none opacity-50' : ''}`}
        >
          <div>
            <label htmlFor="settings-ocr-min-confidence" className="mb-1 block text-content-secondary text-xs">
              {t('settingsAutomation.ocrMinConfidence')}
            </label>
            <Input
              id="settings-ocr-min-confidence"
              type="number"
              min={0}
              max={1}
              step={0.05}
              value={formData.ai_provider.ocr_validation.min_confidence}
              onChange={(e) => onOcrValidationChange('min_confidence', Number(e.target.value))}
            />
            <FieldHint>{t('settingsAutomation.ocrMinConfidenceHint')}</FieldHint>
          </div>
          <div>
            <label htmlFor="settings-ocr-max-invalid-ratio" className="mb-1 block text-content-secondary text-xs">
              {t('settingsAutomation.ocrMaxInvalidRatio')}
            </label>
            <Input
              id="settings-ocr-max-invalid-ratio"
              type="number"
              min={0}
              max={1}
              step={0.05}
              value={formData.ai_provider.ocr_validation.max_invalid_ratio}
              onChange={(e) => onOcrValidationChange('max_invalid_ratio', Number(e.target.value))}
            />
            <FieldHint>{t('settingsAutomation.ocrMaxInvalidRatioHint')}</FieldHint>
          </div>
        </div>
      </div>
    </>
  )
}
