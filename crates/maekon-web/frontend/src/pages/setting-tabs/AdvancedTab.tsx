import { useTranslation } from 'react-i18next'
import type { AppSettings } from '../../api/client'
import { Card, CardTitle, GuidancePanel, Input } from '../../components/ui'
import { colors, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { useLoadedFormData, useSettingsFormContext } from '../settings/SettingsFormContext'
import ToggleRow from './ToggleRow'

function SectionLabel({ children, htmlFor }: { children: React.ReactNode; htmlFor?: string }) {
  return (
    <label htmlFor={htmlFor} className={cn('mb-1 block text-sm', colors.text.secondary)}>
      {children}
    </label>
  )
}

function NumberField({
  id,
  label,
  value,
  onChange,
  min,
  max,
  step,
}: {
  id: string
  label: string
  value: number
  onChange: (v: number) => void
  min?: number
  max?: number
  step?: number
}) {
  return (
    <div>
      <SectionLabel htmlFor={id}>{label}</SectionLabel>
      <Input
        id={id}
        type="number"
        value={value}
        min={min}
        max={max}
        step={step}
        onChange={(e) => onChange(Number(e.target.value))}
        className="w-full"
      />
    </div>
  )
}

export default function AdvancedTab() {
  const { t } = useTranslation()
  const { form } = useSettingsFormContext()
  const formData = useLoadedFormData()

  const handleChange = <K extends keyof AppSettings>(section: K, field: string, value: unknown) => {
    form.setFormData((prev) => {
      if (!prev) return prev
      const sectionData = prev[section]
      if (typeof sectionData === 'object' && sectionData !== null) {
        return { ...prev, [section]: { ...sectionData, [field]: value } }
      }
      return prev
    })
  }

  // G2a (#8059): the three config flags that make up the discoverable "AI
  // features" bundle — local embedding, the on-device AI daily-digest
  // narrative, and semantic search (which needs embedding wired). The master
  // toggle flips all three atomically (on → all on; off → all off); the
  // individual power-user toggles below stay in sync. Checkbox has no
  // indeterminate state, so the master reads ON only when all three are on.
  const aiFeaturesAllOn =
    formData.analysis.enabled && formData.analysis.embedding_enabled && formData.analysis.llm_summary_enabled

  const handleEnableAiFeatures = (value: boolean) => {
    form.setFormData((prev) => {
      if (!prev) return prev
      return {
        ...prev,
        analysis: {
          ...prev.analysis,
          enabled: value,
          embedding_enabled: value,
          llm_summary_enabled: value,
        },
      }
    })
  }

  return (
    <div className="space-y-6">
      <GuidancePanel
        title={t('settings.guidance.advanced.title')}
        description={t('settings.guidance.advanced.description')}
        items={[
          {
            title: t('settings.guidance.advanced.runtime.title'),
            description: t('settings.guidance.advanced.runtime.description'),
          },
          {
            title: t('settings.guidance.advanced.network.title'),
            description: t('settings.guidance.advanced.network.description'),
          },
          {
            title: t('settings.guidance.advanced.sync.title'),
            description: t('settings.guidance.advanced.sync.description'),
          },
        ]}
      />

      {/* AI Session */}
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.advanced.aiSession', 'AI Session')}</CardTitle>
        <div className="grid grid-cols-2 gap-4">
          <NumberField
            id="ai-session-max-concurrent"
            label={t('advancedTab.maxConcurrentSessions')}
            value={formData.ai_session.max_concurrent_sessions}
            onChange={(v) => handleChange('ai_session', 'max_concurrent_sessions', v)}
            min={1}
            max={10}
          />
          <NumberField
            id="ai-session-idle-timeout"
            label={t('advancedTab.idleTimeoutSeconds')}
            value={formData.ai_session.idle_timeout_secs}
            onChange={(v) => handleChange('ai_session', 'idle_timeout_secs', v)}
            min={30}
          />
          <NumberField
            id="ai-session-timeout"
            label={t('advancedTab.sessionTimeoutSeconds')}
            value={formData.ai_session.session_timeout_secs}
            onChange={(v) => handleChange('ai_session', 'session_timeout_secs', v)}
            min={60}
          />
          <NumberField
            id="ai-session-retries"
            label={t('advancedTab.maxRetries')}
            value={formData.ai_session.max_retries}
            onChange={(v) => handleChange('ai_session', 'max_retries', v)}
            min={0}
            max={10}
          />
          <NumberField
            id="ai-session-history"
            label={t('advancedTab.maxHistoryTurns')}
            value={formData.ai_session.max_history_turns}
            onChange={(v) => handleChange('ai_session', 'max_history_turns', v)}
            min={10}
          />
          <NumberField
            id="ai-session-health"
            label={t('advancedTab.healthCheckIntervalSeconds')}
            value={formData.ai_session.health_check_interval_secs}
            onChange={(v) => handleChange('ai_session', 'health_check_interval_secs', v)}
            min={5}
          />
        </div>
      </Card>

      {/* Suggestion */}
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.advanced.suggestion', 'Suggestions')}</CardTitle>
        <ToggleRow
          label={t('advancedTab.enableSuggestions')}
          description={t('advancedTab.enableSuggestionsDescription')}
          checked={formData.suggestion.enabled}
          onChange={(v) => handleChange('suggestion', 'enabled', v)}
        />
      </Card>

      {/* Indicator */}
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.advanced.indicator', 'Screen Indicator')}</CardTitle>
        <div className="space-y-4">
          <ToggleRow
            label={t('advancedTab.showBorder')}
            description={t('advancedTab.showBorderDescription')}
            checked={formData.indicator.show_border}
            onChange={(v) => handleChange('indicator', 'show_border', v)}
          />
          <ToggleRow
            label={t('advancedTab.showPanel')}
            description={t('advancedTab.showPanelDescription')}
            checked={formData.indicator.show_panel}
            onChange={(v) => handleChange('indicator', 'show_panel', v)}
          />
          <NumberField
            id="indicator-opacity"
            label={t('advancedTab.borderOpacity')}
            value={formData.indicator.border_opacity}
            onChange={(v) => handleChange('indicator', 'border_opacity', v)}
            min={0}
            max={1}
            step={0.1}
          />
        </div>
      </Card>

      {/* Analysis */}
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.advanced.analysis', 'Analysis Pipeline')}</CardTitle>
        <div className="space-y-4">
          {/* G2a (#8059): master "Enable AI features" toggle — flips analysis,
              embedding, and llm_summary together so the built-but-hidden AI
              features (semantic search, AI daily-digest narrative) are
              discoverable in one click. Everything runs on-device. */}
          <div className={cn('rounded-lg border border-brand-signal/40 bg-brand-signal/5 p-4', motion.colors)}>
            <ToggleRow
              label={t('advancedTab.aiFeaturesMaster')}
              description={t('advancedTab.aiFeaturesMasterDescription')}
              checked={aiFeaturesAllOn}
              onChange={handleEnableAiFeatures}
            />
          </div>
          <ToggleRow
            label={t('advancedTab.enableAnalysis')}
            description={t('advancedTab.enableAnalysisDescription')}
            checked={formData.analysis.enabled}
            onChange={(v) => handleChange('analysis', 'enabled', v)}
          />
          <div className="grid grid-cols-2 gap-4">
            <NumberField
              id="analysis-interval"
              label={t('advancedTab.analysisIntervalSeconds')}
              value={formData.analysis.interval_secs}
              onChange={(v) => handleChange('analysis', 'interval_secs', v)}
              min={10}
            />
            <NumberField
              id="analysis-confidence"
              label={t('advancedTab.minConfidence')}
              value={formData.analysis.min_confidence}
              onChange={(v) => handleChange('analysis', 'min_confidence', v)}
              min={0}
              max={1}
              step={0.1}
            />
            <NumberField
              id="analysis-max-suggestions"
              label={t('advancedTab.maxSuggestions')}
              value={formData.analysis.max_suggestions}
              onChange={(v) => handleChange('analysis', 'max_suggestions', v)}
              min={1}
              max={20}
            />
            <NumberField
              id="regime-detection-interval"
              label={t('advancedTab.regimeDetectionIntervalHours')}
              value={formData.analysis.tiered_memory?.regime_detection_interval_hours ?? 2}
              onChange={(v) => handleChange('analysis', 'regime_detection_interval_hours' as never, v)}
              min={1}
              max={24}
            />
          </div>
          <ToggleRow
            label={t('advancedTab.embedding')}
            description={t('advancedTab.embeddingDescription')}
            checked={formData.analysis.embedding_enabled}
            onChange={(v) => handleChange('analysis', 'embedding_enabled', v)}
          />
          <ToggleRow
            label={t('advancedTab.llmSummary')}
            description={t('advancedTab.llmSummaryDescription')}
            checked={formData.analysis.llm_summary_enabled}
            onChange={(v) => handleChange('analysis', 'llm_summary_enabled', v)}
          />
          <ToggleRow
            label={t('advancedTab.guiIntelligence')}
            description={t('advancedTab.guiIntelligenceDescription')}
            checked={formData.analysis.gui_intelligence_enabled}
            onChange={(v) => handleChange('analysis', 'gui_intelligence_enabled', v)}
          />
          <ToggleRow
            label={t('advancedTab.textIntelligence')}
            description={t('advancedTab.textIntelligenceDescription')}
            checked={formData.analysis.text_intelligence_enabled}
            onChange={(v) => handleChange('analysis', 'text_intelligence_enabled', v)}
          />
          <ToggleRow
            label={t('advancedTab.autoTuner')}
            description={t('advancedTab.autoTunerDescription')}
            checked={formData.analysis.auto_tuner_enabled}
            onChange={(v) => handleChange('analysis', 'auto_tuner_enabled', v)}
          />
        </div>
      </Card>

      {/* Network */}
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.advanced.network', 'Network & Server')}</CardTitle>
        <div className="space-y-4">
          <div>
            <SectionLabel htmlFor="network-base-url">{t('advancedTab.serverBaseUrl')}</SectionLabel>
            <Input
              id="network-base-url"
              value={formData.network.server_base_url}
              onChange={(e) => handleChange('network', 'server_base_url', e.target.value)}
              className={cn('w-full', typography.family.mono)}
            />
          </div>
          <NumberField
            id="network-timeout"
            label={t('advancedTab.requestTimeoutMs')}
            value={formData.network.request_timeout_ms}
            onChange={(v) => handleChange('network', 'request_timeout_ms', v)}
            min={1000}
          />
          <ToggleRow
            label={t('advancedTab.grpcEnabled')}
            description={t('advancedTab.grpcEnabledDescription')}
            checked={formData.network.grpc_enabled}
            onChange={(v) => handleChange('network', 'grpc_enabled', v)}
          />
          {formData.network.grpc_enabled && (
            <div>
              <SectionLabel htmlFor="network-grpc-endpoint">{t('advancedTab.grpcEndpoint')}</SectionLabel>
              <Input
                id="network-grpc-endpoint"
                value={formData.network.grpc_endpoint}
                onChange={(e) => handleChange('network', 'grpc_endpoint', e.target.value)}
                className={cn('w-full', typography.family.mono)}
              />
            </div>
          )}
          <ToggleRow
            label={t('advancedTab.tlsEnabled')}
            description={t('advancedTab.tlsEnabledDescription')}
            checked={formData.network.tls_enabled}
            onChange={(v) => handleChange('network', 'tls_enabled', v)}
          />
        </div>
      </Card>

      {/* Coaching */}
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.advanced.coaching', 'Coaching')}</CardTitle>
        <div className="space-y-4">
          <ToggleRow
            label={t('advancedTab.enableCoaching')}
            description={t('advancedTab.enableCoachingDescription')}
            checked={formData.coaching.enabled}
            onChange={(v) => handleChange('coaching', 'enabled', v)}
          />
          <div>
            <SectionLabel htmlFor="coaching-locale">{t('advancedTab.locale')}</SectionLabel>
            <Input
              id="coaching-locale"
              value={formData.coaching.locale}
              onChange={(e) => handleChange('coaching', 'locale', e.target.value)}
              className="w-full"
            />
          </div>
        </div>
      </Card>

      {/* Integration */}
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.advanced.integration', 'Integration')}</CardTitle>
        <div className="space-y-4">
          <ToggleRow
            label={t('advancedTab.enableIntegration')}
            description={t('advancedTab.enableIntegrationDescription')}
            checked={formData.integration.enabled}
            onChange={(v) => handleChange('integration', 'enabled', v)}
          />
          <div className="grid grid-cols-2 gap-4">
            <NumberField
              id="integration-timeout"
              label={t('advancedTab.integrationRequestTimeoutSeconds')}
              value={formData.integration.request_timeout_secs}
              onChange={(v) => handleChange('integration', 'request_timeout_secs', v)}
              min={5}
            />
            <NumberField
              id="integration-sync"
              label={t('advancedTab.integrationSyncIntervalSeconds')}
              value={formData.integration.sync_interval_secs}
              onChange={(v) => handleChange('integration', 'sync_interval_secs', v)}
              min={10}
            />
          </div>
        </div>
      </Card>

      {/* Sync */}
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.advanced.sync', 'Cross-Device Sync')}</CardTitle>
        <div className="space-y-4">
          <ToggleRow
            label={t('advancedTab.enableSync')}
            description={t('advancedTab.enableSyncDescription')}
            checked={formData.sync.enabled}
            onChange={(v) => handleChange('sync', 'enabled', v)}
          />
          {formData.sync.enabled && (
            <>
              <div>
                <SectionLabel htmlFor="sync-device-name">{t('advancedTab.deviceName')}</SectionLabel>
                <Input
                  id="sync-device-name"
                  value={formData.sync.device_name}
                  onChange={(e) => handleChange('sync', 'device_name', e.target.value)}
                  className="w-full"
                />
              </div>
              <NumberField
                id="sync-interval"
                label={t('advancedTab.syncIntervalSeconds')}
                value={formData.sync.interval_secs}
                onChange={(v) => handleChange('sync', 'interval_secs', v)}
                min={30}
              />
              <ToggleRow
                label={t('advancedTab.lanDiscovery')}
                description={t('advancedTab.lanDiscoveryDescription')}
                checked={formData.sync.lan_advertise}
                onChange={(v) => handleChange('sync', 'lan_advertise', v)}
              />
              <ToggleRow
                label={t('advancedTab.payloadCompression')}
                description={t('advancedTab.payloadCompressionDescription')}
                checked={formData.sync.compression_enabled}
                onChange={(v) => handleChange('sync', 'compression_enabled', v)}
              />
            </>
          )}
        </div>
      </Card>
    </div>
  )
}
