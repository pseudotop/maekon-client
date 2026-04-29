import { useTranslation } from 'react-i18next'
import type { AppSettings } from '../../api/client'
import { Card, CardTitle, GuidancePanel, Input } from '../../components/ui'
import { colors, typography } from '../../styles/tokens'
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
}: {
  id: string
  label: string
  value: number
  onChange: (v: number) => void
  min?: number
  max?: number
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
            label="Max concurrent sessions"
            value={formData.ai_session.max_concurrent_sessions}
            onChange={(v) => handleChange('ai_session', 'max_concurrent_sessions', v)}
            min={1}
            max={10}
          />
          <NumberField
            id="ai-session-idle-timeout"
            label="Idle timeout (seconds)"
            value={formData.ai_session.idle_timeout_secs}
            onChange={(v) => handleChange('ai_session', 'idle_timeout_secs', v)}
            min={30}
          />
          <NumberField
            id="ai-session-timeout"
            label="Session timeout (seconds)"
            value={formData.ai_session.session_timeout_secs}
            onChange={(v) => handleChange('ai_session', 'session_timeout_secs', v)}
            min={60}
          />
          <NumberField
            id="ai-session-retries"
            label="Max retries"
            value={formData.ai_session.max_retries}
            onChange={(v) => handleChange('ai_session', 'max_retries', v)}
            min={0}
            max={10}
          />
          <NumberField
            id="ai-session-history"
            label="Max history turns"
            value={formData.ai_session.max_history_turns}
            onChange={(v) => handleChange('ai_session', 'max_history_turns', v)}
            min={10}
          />
          <NumberField
            id="ai-session-health"
            label="Health check interval (seconds)"
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
          label="Enable suggestions"
          description="Receive AI-powered suggestions based on your activity"
          checked={formData.suggestion.enabled}
          onChange={(v) => handleChange('suggestion', 'enabled', v)}
        />
      </Card>

      {/* Indicator */}
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.advanced.indicator', 'Screen Indicator')}</CardTitle>
        <div className="space-y-4">
          <ToggleRow
            label="Show border"
            description="Display a colored border around the screen when monitoring"
            checked={formData.indicator.show_border}
            onChange={(v) => handleChange('indicator', 'show_border', v)}
          />
          <ToggleRow
            label="Show panel"
            description="Display the tracking status panel"
            checked={formData.indicator.show_panel}
            onChange={(v) => handleChange('indicator', 'show_panel', v)}
          />
          <NumberField
            id="indicator-opacity"
            label="Border opacity (0.0 - 1.0)"
            value={formData.indicator.border_opacity}
            onChange={(v) => handleChange('indicator', 'border_opacity', v)}
            min={0}
            max={1}
          />
        </div>
      </Card>

      {/* Analysis */}
      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.advanced.analysis', 'Analysis Pipeline')}</CardTitle>
        <div className="space-y-4">
          <ToggleRow
            label="Enable analysis"
            description="Run LLM analysis on activity segments"
            checked={formData.analysis.enabled}
            onChange={(v) => handleChange('analysis', 'enabled', v)}
          />
          <div className="grid grid-cols-2 gap-4">
            <NumberField
              id="analysis-interval"
              label="Analysis interval (seconds)"
              value={formData.analysis.interval_secs}
              onChange={(v) => handleChange('analysis', 'interval_secs', v)}
              min={10}
            />
            <NumberField
              id="analysis-confidence"
              label="Min confidence (0.0 - 1.0)"
              value={formData.analysis.min_confidence}
              onChange={(v) => handleChange('analysis', 'min_confidence', v)}
              min={0}
              max={1}
            />
            <NumberField
              id="analysis-max-suggestions"
              label="Max suggestions"
              value={formData.analysis.max_suggestions}
              onChange={(v) => handleChange('analysis', 'max_suggestions', v)}
              min={1}
              max={20}
            />
            <NumberField
              id="regime-detection-interval"
              label="Regime detection interval (hours)"
              value={formData.analysis.tiered_memory?.regime_detection_interval_hours ?? 2}
              onChange={(v) => handleChange('analysis', 'regime_detection_interval_hours' as never, v)}
              min={1}
              max={24}
            />
          </div>
          <ToggleRow
            label="Embedding"
            description="Enable vector embedding for semantic search"
            checked={formData.analysis.embedding_enabled}
            onChange={(v) => handleChange('analysis', 'embedding_enabled', v)}
          />
          <ToggleRow
            label="GUI intelligence"
            description="Detect and aggregate GUI interaction patterns"
            checked={formData.analysis.gui_intelligence_enabled}
            onChange={(v) => handleChange('analysis', 'gui_intelligence_enabled', v)}
          />
          <ToggleRow
            label="Text intelligence"
            description="Extract and classify text patterns from activity"
            checked={formData.analysis.text_intelligence_enabled}
            onChange={(v) => handleChange('analysis', 'text_intelligence_enabled', v)}
          />
          <ToggleRow
            label="Auto-tuner"
            description="EMA-based drift detection and automatic re-clustering of behavioral regimes"
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
            <SectionLabel htmlFor="network-base-url">Server base URL</SectionLabel>
            <Input
              id="network-base-url"
              value={formData.network.server_base_url}
              onChange={(e) => handleChange('network', 'server_base_url', e.target.value)}
              className={cn('w-full', typography.family.mono)}
            />
          </div>
          <NumberField
            id="network-timeout"
            label="Request timeout (ms)"
            value={formData.network.request_timeout_ms}
            onChange={(v) => handleChange('network', 'request_timeout_ms', v)}
            min={1000}
          />
          <ToggleRow
            label="gRPC enabled"
            description="Use gRPC for server communication (requires server support)"
            checked={formData.network.grpc_enabled}
            onChange={(v) => handleChange('network', 'grpc_enabled', v)}
          />
          {formData.network.grpc_enabled && (
            <div>
              <SectionLabel htmlFor="network-grpc-endpoint">gRPC endpoint</SectionLabel>
              <Input
                id="network-grpc-endpoint"
                value={formData.network.grpc_endpoint}
                onChange={(e) => handleChange('network', 'grpc_endpoint', e.target.value)}
                className={cn('w-full', typography.family.mono)}
              />
            </div>
          )}
          <ToggleRow
            label="TLS enabled"
            description="Encrypt outbound connections with TLS"
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
            label="Enable coaching"
            description="Proactive productivity coaching and goal tracking"
            checked={formData.coaching.enabled}
            onChange={(v) => handleChange('coaching', 'enabled', v)}
          />
          <div>
            <SectionLabel htmlFor="coaching-locale">Locale</SectionLabel>
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
            label="Enable integration"
            description="Connect to external integration hub"
            checked={formData.integration.enabled}
            onChange={(v) => handleChange('integration', 'enabled', v)}
          />
          <div className="grid grid-cols-2 gap-4">
            <NumberField
              id="integration-timeout"
              label="Request timeout (seconds)"
              value={formData.integration.request_timeout_secs}
              onChange={(v) => handleChange('integration', 'request_timeout_secs', v)}
              min={5}
            />
            <NumberField
              id="integration-sync"
              label="Sync interval (seconds)"
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
            label="Enable sync"
            description="Synchronize data across devices"
            checked={formData.sync.enabled}
            onChange={(v) => handleChange('sync', 'enabled', v)}
          />
          {formData.sync.enabled && (
            <>
              <div>
                <SectionLabel htmlFor="sync-device-name">Device name</SectionLabel>
                <Input
                  id="sync-device-name"
                  value={formData.sync.device_name}
                  onChange={(e) => handleChange('sync', 'device_name', e.target.value)}
                  className="w-full"
                />
              </div>
              <NumberField
                id="sync-interval"
                label="Sync interval (seconds)"
                value={formData.sync.interval_secs}
                onChange={(v) => handleChange('sync', 'interval_secs', v)}
                min={30}
              />
              <ToggleRow
                label="LAN discovery"
                description="Advertise this device on the local network for peer sync"
                checked={formData.sync.lan_advertise}
                onChange={(v) => handleChange('sync', 'lan_advertise', v)}
              />
              <ToggleRow
                label="Payload compression"
                description="Compress changeset payloads before encryption to reduce bandwidth"
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
