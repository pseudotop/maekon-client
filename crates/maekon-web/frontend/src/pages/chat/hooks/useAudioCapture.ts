import type React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { addToast } from '../../../hooks/useToast'
import { IS_LINUX, IS_MAC, IS_WINDOWS } from '../../../utils/platform'
import { errorMessage, ipc } from '../utils'

/** The i18next translate function, as returned by `useTranslation()`. */
type MicTranslate = ReturnType<typeof useTranslation>['t']

/**
 * #8053: OS-specific path a user follows to grant microphone permission. Detected
 * from the existing `platform` util (no new dependency). Falls back to a generic
 * hint when the platform is unknown (e.g. standalone browser mode).
 */
function micPermissionPath(t: MicTranslate): string {
  if (IS_MAC) return t('chat.mic_permission_path_macos', 'System Settings › Privacy & Security › Microphone')
  if (IS_WINDOWS) return t('chat.mic_permission_path_windows', 'Settings › Privacy & security › Microphone')
  if (IS_LINUX)
    return t('chat.mic_permission_path_linux', 'your desktop privacy settings (varies by distribution / portal)')
  return t('chat.mic_permission_path_generic', 'your system privacy settings')
}

/**
 * #8053: compose the mic-failure toast — the base error plus OS-aware guidance for
 * granting microphone permission, the dominant cause of a failed capture on
 * macOS/Windows. The guidance is phrased conditionally ("If access was blocked…")
 * so it stays accurate when the real cause is a missing or busy device.
 */
function notifyMicError(t: MicTranslate, err: unknown): void {
  const base = errorMessage(err, t('chat.mic_error', 'Microphone not available'))
  const hint = t('chat.mic_permission_hint', 'If microphone access was blocked, enable it in:')
  addToast('error', `${base} — ${hint} ${micPermissionPath(t)}`, 8000)
}

export function useAudioCapture(isReadOnly: boolean, setInput: React.Dispatch<React.SetStateAction<string>>) {
  const { t } = useTranslation()
  const [audioAvailable, setAudioAvailable] = useState(true)
  const [audioTooltip, setAudioTooltip] = useState('Hold to speak')
  const [micMode, setMicMode] = useState<'push_to_talk' | 'voice_activity'>('push_to_talk')
  const [vadState, setVadState] = useState<'idle' | 'listening' | 'speech' | 'transcribing'>('idle')
  const [recording, setRecording] = useState(false)
  const [transcribing, setTranscribing] = useState(false)
  const recordingRef = useRef(false)

  // Clean up active audio capture on unmount
  useEffect(() => {
    return () => {
      if (recordingRef.current) {
        recordingRef.current = false
        ipc('stop_and_transcribe').catch(() => {})
      }
      ipc('stop_vad_listening').catch(() => {})
    }
  }, [])

  // Check audio status
  useEffect(() => {
    ;(async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        // #7600: COMPILE-capability gate checked FIRST. `maekon-audio` is
        // compiled OUT of the shipped `grpc,windows-sandbox` release build, so
        // `get_audio_status` alone is not a truthful signal — its
        // `model_status.state` can never reach `ready` there (the model
        // downloader is None), which previously left the mic button showing
        // "Download model in Settings" — an actionable-looking hint that leads
        // to a doomed download. Short-circuit with an honest tooltip instead.
        const capabilities = await invoke<{ audio_compiled?: boolean }>('get_feature_capabilities')
        if (capabilities?.audio_compiled !== true) {
          setAudioAvailable(false)
          setAudioTooltip(t('chat.audio_not_compiled', 'Audio is not available in this build'))
          return
        }
        // #8651: model readiness is only a capability signal. Consent is a
        // separate fail-closed prerequisite and must shape the visible affordance.
        const consent = await invoke<{
          status: string
          permissions?: { microphone?: boolean }
        }>('get_consent')
        if (consent.status !== 'Valid' || consent.permissions?.microphone !== true) {
          setAudioAvailable(false)
          setAudioTooltip(t('chat.mic_consent_required', 'Enable microphone consent in Privacy'))
          return
        }
        const status = await invoke<{
          enabled: boolean
          model_status: { state: string }
          stt_provider_loaded: boolean
          mic_input_mode?: string
        }>('get_audio_status')
        if (!status.enabled) {
          setAudioAvailable(false)
          setAudioTooltip(t('chat.audio_disabled', 'Audio disabled in Settings'))
        } else if (status.model_status.state !== 'ready') {
          setAudioAvailable(false)
          setAudioTooltip(t('chat.model_needed', 'Download model in Settings'))
        } else {
          setAudioAvailable(true)
          const mode = (
            status.mic_input_mode === 'voice_activity' ? 'voice_activity' : 'push_to_talk'
          ) as typeof micMode
          setMicMode(mode)
          setAudioTooltip(
            mode === 'voice_activity'
              ? t('chat.mic_vad_tooltip', 'Click to toggle listening')
              : t('chat.mic_tooltip', 'Hold to speak'),
          )
        }
      } catch {
        // not in Tauri
      }
    })()
  }, [t])

  // VAD event listeners
  useEffect(() => {
    if (micMode !== 'voice_activity') return
    let cleanup: (() => void) | undefined
    ;(async () => {
      try {
        const { listen } = await import('@tauri-apps/api/event')
        const unlisten1 = await listen<{ state: string; reason?: string }>('vad-state-changed', (event) => {
          const s = event.payload.state as typeof vadState
          setVadState(s)
          if (s === 'transcribing') setTranscribing(true)
          else setTranscribing(false)
          // Notify the user when the privacy gate has forcibly stopped the microphone
          if (s === 'idle' && event.payload.reason === 'privacy_gate_closed') {
            addToast('warning', t('chat.mic_privacy_stopped', 'Microphone stopped — privacy gate active'), 5000)
          }
        })
        const unlisten2 = await listen<{ text: string; duration_secs: number; processing_secs: number }>(
          'vad-transcription-result',
          (event) => {
            if (event.payload.text) {
              setInput((prev) => (prev ? `${prev} ` : '') + event.payload.text)
            }
          },
        )
        cleanup = () => {
          unlisten1()
          unlisten2()
        }
      } catch {
        // not in Tauri
      }
    })()
    return () => cleanup?.()
  }, [micMode, setInput, t])

  // PTT mode: hold-to-speak handlers
  const handleMicDown = useCallback(
    async (e?: React.SyntheticEvent) => {
      if (micMode === 'voice_activity') return
      if (e?.nativeEvent instanceof TouchEvent) e.preventDefault()
      if (isReadOnly || recordingRef.current || transcribing) return
      recordingRef.current = true
      setRecording(true)
      try {
        await ipc('start_audio_capture')
      } catch (err) {
        recordingRef.current = false
        setRecording(false)
        notifyMicError(t, err)
      }
    },
    [isReadOnly, transcribing, t, micMode],
  )

  const handleMicUp = useCallback(async () => {
    if (micMode === 'voice_activity') return
    if (!recordingRef.current) return
    recordingRef.current = false
    setRecording(false)
    setTranscribing(true)
    try {
      const result = await ipc<{ text: string }>('stop_and_transcribe')
      if (result.text) {
        setInput((prev) => (prev ? `${prev} ` : '') + result.text)
      }
    } catch (e) {
      addToast('error', errorMessage(e, t('chat.stt_error', 'Transcription failed')), 5000)
    } finally {
      setTranscribing(false)
    }
  }, [t, micMode, setInput])

  // VAD mode: click to toggle listening
  const handleVadToggle = useCallback(async () => {
    if (isReadOnly || transcribing) return
    if (vadState === 'idle') {
      try {
        await ipc('start_vad_listening')
      } catch (err) {
        notifyMicError(t, err)
      }
    } else {
      try {
        await ipc('stop_vad_listening')
        setVadState('idle')
      } catch {
        // ignore
      }
    }
  }, [isReadOnly, transcribing, vadState, t])

  return {
    audioAvailable,
    audioTooltip,
    micMode,
    vadState,
    recording,
    transcribing,
    handleMicDown,
    handleMicUp,
    handleVadToggle,
  }
}
