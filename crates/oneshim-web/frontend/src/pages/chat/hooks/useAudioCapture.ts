import type React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { addToast } from '../../../hooks/useToast'
import { errorMessage, ipc } from '../utils'

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
        const unlisten1 = await listen<{ state: string }>('vad-state-changed', (event) => {
          const s = event.payload.state as typeof vadState
          setVadState(s)
          if (s === 'transcribing') setTranscribing(true)
          else setTranscribing(false)
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
  }, [micMode, setInput])

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
        addToast('error', errorMessage(err, t('chat.mic_error', 'Microphone not available')), 5000)
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
        addToast('error', errorMessage(err, t('chat.mic_error', 'Microphone not available')), 5000)
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
