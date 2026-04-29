import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Badge, Button, Checkbox, GuidancePanel, Spinner } from '../../components/ui'
import { colors, radius, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { useSettingsFormContext } from '../settings/SettingsFormContext'

const ipc = async <T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

type ModelSize = 'tiny' | 'base' | 'small' | 'medium'
type AudioStatusResponse = {
  enabled: boolean
  selected_model: ModelSize
  model_status:
    | { state: 'not_installed' }
    | { state: 'downloading'; progress_pct: number | null; bytes_downloaded: number; total_bytes: number | null }
    | { state: 'ready'; path: string; size_bytes: number }
    | { state: 'error'; message: string }
  stt_provider_loaded: boolean
}

const MODEL_LABELS: Record<ModelSize, string> = {
  tiny: 'Tiny (~75 MB)',
  base: 'Base (~142 MB)',
  small: 'Small (~466 MB)',
  medium: 'Medium (~1.5 GB)',
}

export default function AudioTab() {
  const { t } = useTranslation()
  const { form } = useSettingsFormContext()
  const formData = form.formData

  const handleAudioChange = (field: string, value: unknown) => {
    form.setFormData((prev) => {
      if (!prev) return prev
      return { ...prev, audio: { ...prev.audio, [field]: value } }
    })
  }
  const [audioStatus, setAudioStatus] = useState<AudioStatusResponse | null>(null)
  const [downloading, setDownloading] = useState(false)

  const fetchStatus = useCallback(async () => {
    try {
      const status = await ipc<AudioStatusResponse>('get_audio_status')
      setAudioStatus(status)
      setDownloading(status.model_status.state === 'downloading')
    } catch {
      // Tauri not available (standalone mode)
    }
  }, [])

  useEffect(() => {
    fetchStatus()
  }, [fetchStatus])

  useEffect(() => {
    let cleanup: (() => void) | undefined
    ;(async () => {
      try {
        const { listen } = await import('@tauri-apps/api/event')
        const unlisten1 = await listen<{
          progress_pct: number | null
          bytes_downloaded: number
          total_bytes: number | null
        }>('audio-model-progress', (event) => {
          setAudioStatus((prev) =>
            prev
              ? {
                  ...prev,
                  model_status: {
                    state: 'downloading' as const,
                    progress_pct: event.payload.progress_pct,
                    bytes_downloaded: event.payload.bytes_downloaded,
                    total_bytes: event.payload.total_bytes,
                  },
                }
              : prev,
          )
        })
        const unlisten2 = await listen('audio-model-complete', () => {
          setDownloading(false)
          fetchStatus()
        })
        const unlisten3 = await listen('audio-model-error', () => {
          setDownloading(false)
          fetchStatus()
        })
        cleanup = () => {
          unlisten1()
          unlisten2()
          unlisten3()
        }
      } catch {
        // not in Tauri
      }
    })()
    return () => cleanup?.()
  }, [fetchStatus])

  const audio = formData?.audio
  const enabled = audio?.enabled ?? false
  const modelSize = (audio?.model_size ?? 'base') as ModelSize
  const language = audio?.language ?? 'auto'
  const modelState = audioStatus?.model_status?.state ?? 'not_installed'
  const sttProvider = (audio?.stt_provider ?? 'local') as string
  const cloudApiKey = audio?.cloud_api_key ?? ''
  const cloudEndpoint = audio?.cloud_stt_endpoint ?? 'https://api.openai.com/v1/audio/transcriptions'
  const cloudTimeoutSecs = audio?.cloud_timeout_secs ?? 30
  const micInputMode = audio?.mic_input_mode ?? 'push_to_talk'
  const vadThreshold = audio?.vad_threshold ?? 0.02
  const vadSilenceMs = audio?.vad_silence_ms ?? 800

  const handleDownload = async () => {
    setDownloading(true)
    try {
      await ipc('download_whisper_model', { modelSize })
    } catch {
      setDownloading(false)
      fetchStatus()
    }
  }

  const handleCancel = async () => {
    try {
      await ipc('cancel_model_download')
    } catch {
      // ignore
    }
  }

  const handleDelete = async () => {
    if (!confirm(t('settings.audio.delete_confirm', 'Delete the downloaded model? You can re-download it later.')))
      return
    try {
      await ipc('delete_whisper_model', { modelSize })
      fetchStatus()
    } catch {
      // ignore
    }
  }

  const handleReload = async () => {
    try {
      await ipc('reload_stt_engine')
      fetchStatus()
    } catch {
      // ignore
    }
  }

  return (
    <div className="space-y-6">
      <h3 className={cn(typography.h3, colors.text.primary)}>{t('settings.audio.title', 'Audio & Speech-to-Text')}</h3>
      <GuidancePanel
        title={t('settings.guidance.audio.title')}
        description={t('settings.guidance.audio.description')}
        items={[
          {
            title: t('settings.guidance.audio.provider.title'),
            description: t('settings.guidance.audio.provider.description'),
          },
          {
            title: t('settings.guidance.audio.model.title'),
            description: t('settings.guidance.audio.model.description'),
          },
          {
            title: t('settings.guidance.audio.input.title'),
            description: t('settings.guidance.audio.input.description'),
          },
        ]}
      />

      <Checkbox
        checked={enabled}
        onChange={(e) => handleAudioChange('enabled', e.target.checked)}
        label={t('settings.audio.enable', 'Enable audio capture and STT')}
      />

      <div className="space-y-2">
        <label htmlFor="audio-model-size" className={cn(typography.label, colors.text.secondary)}>
          {t('settings.audio.model', 'Whisper Model')}
        </label>
        <select
          id="audio-model-size"
          value={modelSize}
          onChange={(e) => handleAudioChange('model_size', e.target.value)}
          disabled={downloading || !enabled}
          className={cn('w-full border bg-surface-base px-3 py-2 text-sm', radius.md, colors.text.primary)}
        >
          {(Object.entries(MODEL_LABELS) as [ModelSize, string][]).map(([key, label]) => (
            <option key={key} value={key}>
              {label}
            </option>
          ))}
        </select>
      </div>

      <div className="space-y-2">
        <span className={cn(typography.label, colors.text.secondary)}>
          {t('settings.audio.status', 'Model Status')}
        </span>
        <div className="flex items-center gap-3">
          {modelState === 'not_installed' && (
            <Badge size="sm" className="bg-surface-muted text-content-secondary">
              {t('settings.audio.not_installed', 'Not installed')}
            </Badge>
          )}
          {modelState === 'downloading' && audioStatus?.model_status.state === 'downloading' && (
            <div className="flex items-center gap-2">
              <Spinner />
              <span className="text-sm">
                {audioStatus.model_status.progress_pct != null
                  ? `${audioStatus.model_status.progress_pct}%`
                  : t('settings.audio.downloading', 'Downloading...')}
              </span>
            </div>
          )}
          {modelState === 'ready' && (
            <Badge color="success" size="sm">
              {t('settings.audio.ready', 'Ready')}
            </Badge>
          )}
          {modelState === 'error' && audioStatus?.model_status.state === 'error' && (
            <Badge color="error" size="sm">
              {audioStatus.model_status.message}
            </Badge>
          )}
        </div>
      </div>

      <div className="flex gap-2">
        {downloading ? (
          <Button variant="secondary" size="sm" onClick={handleCancel}>
            {t('settings.audio.cancel', 'Cancel')}
          </Button>
        ) : (
          <Button variant="primary" size="sm" onClick={handleDownload} disabled={!enabled}>
            {modelState === 'ready'
              ? t('settings.audio.redownload', 'Re-download')
              : t('settings.audio.download', 'Download')}
          </Button>
        )}
        {modelState === 'ready' && !downloading && (
          <>
            <Button variant="secondary" size="sm" onClick={handleReload}>
              {t('settings.audio.reload', 'Reload Engine')}
            </Button>
            <Button variant="danger" size="sm" onClick={handleDelete}>
              {t('settings.audio.delete', 'Delete')}
            </Button>
          </>
        )}
      </div>

      <div className="space-y-2">
        <label htmlFor="audio-language" className={cn(typography.label, colors.text.secondary)}>
          {t('settings.audio.language', 'STT Language')}
        </label>
        <select
          id="audio-language"
          value={language}
          onChange={(e) => handleAudioChange('language', e.target.value)}
          disabled={!enabled}
          className={cn('w-full border bg-surface-base px-3 py-2 text-sm', radius.md, colors.text.primary)}
        >
          <option value="auto">Auto-detect</option>
          <option value="en">English</option>
          <option value="ko">한국어</option>
        </select>
      </div>

      {/* Input Mode */}
      <fieldset className="space-y-2">
        <legend className={cn(typography.label, colors.text.secondary)}>
          {t('settings.audio.input_mode', 'Input Mode')}
        </legend>
        <div className="flex gap-4">
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="mic_input_mode"
              value="push_to_talk"
              checked={micInputMode === 'push_to_talk'}
              onChange={() => handleAudioChange('mic_input_mode', 'push_to_talk')}
            />
            <span className={colors.text.primary}>{t('settings.audio.ptt', 'Push-to-Talk')}</span>
          </label>
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="mic_input_mode"
              value="voice_activity"
              checked={micInputMode === 'voice_activity'}
              onChange={() => handleAudioChange('mic_input_mode', 'voice_activity')}
            />
            <span className={colors.text.primary}>{t('settings.audio.vad', 'Voice Activity')}</span>
          </label>
        </div>
      </fieldset>

      {/* VAD Settings (shown when Voice Activity selected) */}
      {micInputMode === 'voice_activity' && (
        <div className="space-y-4 rounded-lg border border-muted p-4">
          <div className="space-y-2">
            <label htmlFor="audio-vad-threshold" className={cn(typography.label, colors.text.secondary)}>
              {t('settings.audio.vad_sensitivity', 'VAD Sensitivity')}
            </label>
            <div className="flex items-center gap-3">
              <input
                id="audio-vad-threshold"
                type="range"
                min="0.005"
                max="0.1"
                step="0.005"
                value={vadThreshold}
                onChange={(e) => handleAudioChange('vad_threshold', Number.parseFloat(e.target.value))}
                className="flex-1"
              />
              <span className={cn('w-12 text-right text-sm tabular-nums', colors.text.secondary)}>
                {vadThreshold.toFixed(3)}
              </span>
            </div>
            <p className={cn(typography.caption, colors.text.tertiary)}>
              {t(
                'settings.audio.vad_sensitivity_hint',
                'Lower = more sensitive. Increase if background noise triggers false detections.',
              )}
            </p>
          </div>
          <div className="space-y-2">
            <label htmlFor="audio-vad-silence" className={cn(typography.label, colors.text.secondary)}>
              {t('settings.audio.vad_silence', 'Silence Duration (ms)')}
            </label>
            <input
              id="audio-vad-silence"
              type="number"
              min="200"
              max="3000"
              step="100"
              value={vadSilenceMs}
              onChange={(e) => handleAudioChange('vad_silence_ms', Number.parseInt(e.target.value, 10) || 800)}
              className={cn('w-32 border bg-surface-base px-3 py-2 text-sm', radius.md, colors.text.primary)}
            />
            <p className={cn(typography.caption, colors.text.tertiary)}>
              {t('settings.audio.vad_silence_hint', 'How long to wait in silence before ending an utterance.')}
            </p>
          </div>
        </div>
      )}

      {/* STT Provider */}
      <fieldset className="space-y-2">
        <legend className={cn(typography.label, colors.text.secondary)}>
          {t('settings.audio.stt_provider', 'STT Provider')}
        </legend>
        <div className="flex gap-4">
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="stt_provider"
              value="local"
              checked={sttProvider === 'local'}
              onChange={() => handleAudioChange('stt_provider', 'local')}
            />
            <span className={colors.text.primary}>{t('settings.audio.local', 'Local (Whisper)')}</span>
          </label>
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="stt_provider"
              value="cloud"
              checked={sttProvider === 'cloud'}
              onChange={() => handleAudioChange('stt_provider', 'cloud')}
            />
            <span className={colors.text.primary}>{t('settings.audio.cloud', 'Cloud (OpenAI)')}</span>
          </label>
        </div>
      </fieldset>

      {/* Cloud STT Settings (shown when Cloud selected) */}
      {sttProvider === 'cloud' && (
        <div className="space-y-4 rounded-lg border border-muted p-4">
          <div className="space-y-2">
            <label htmlFor="cloud-api-key" className={cn(typography.label, colors.text.secondary)}>
              {t('settings.audio.api_key', 'API Key')}
            </label>
            <input
              id="cloud-api-key"
              type="password"
              value={cloudApiKey}
              onChange={(e) => handleAudioChange('cloud_api_key', e.target.value)}
              placeholder="sk-..."
              className={cn('w-full border bg-surface-base px-3 py-2 text-sm', radius.md, colors.text.primary)}
            />
            <p className={cn(typography.caption, colors.text.tertiary)}>
              {t('settings.audio.api_key_hint', 'Your API key is stored locally and sent directly to the provider.')}
            </p>
          </div>
          <div className="space-y-2">
            <label htmlFor="cloud-endpoint" className={cn(typography.label, colors.text.secondary)}>
              {t('settings.audio.endpoint', 'Endpoint URL')}
            </label>
            <input
              id="cloud-endpoint"
              type="url"
              value={cloudEndpoint}
              onChange={(e) => handleAudioChange('cloud_stt_endpoint', e.target.value)}
              placeholder="https://api.openai.com/v1/audio/transcriptions"
              className={cn('w-full border bg-surface-base px-3 py-2 text-sm', radius.md, colors.text.primary)}
            />
            <p className={cn(typography.caption, colors.text.tertiary)}>
              {t('settings.audio.endpoint_hint', 'OpenAI-compatible transcription endpoint.')}
            </p>
          </div>
          <div className="space-y-2">
            <label htmlFor="cloud-timeout" className={cn(typography.label, colors.text.secondary)}>
              {t('settings.audio.cloud_timeout', 'Timeout (sec)')}
            </label>
            <input
              id="cloud-timeout"
              type="number"
              min="5"
              max="120"
              step="5"
              value={cloudTimeoutSecs}
              onChange={(e) => handleAudioChange('cloud_timeout_secs', Number.parseInt(e.target.value, 10) || 30)}
              className={cn('w-32 border bg-surface-base px-3 py-2 text-sm', radius.md, colors.text.primary)}
            />
          </div>
        </div>
      )}
    </div>
  )
}
