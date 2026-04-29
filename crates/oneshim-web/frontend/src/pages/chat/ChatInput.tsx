import { Lightbulb, Loader2, Mic, Paperclip, Send, X } from 'lucide-react'
import type React from 'react'
import { useCallback, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '../../components/ui'
import { colors, iconSize, interaction, radius } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface ChatInputProps {
  input: string
  setInput: React.Dispatch<React.SetStateAction<string>>
  attachments: Array<{ name: string; type: string; data: string }>
  setAttachments: React.Dispatch<React.SetStateAction<Array<{ name: string; type: string; data: string }>>>
  isReadOnly: boolean
  sending: boolean
  sendDisabled: boolean
  onSend: () => void
  // Suggestion props
  onRequestSuggestions?: () => void
  requestingSuggestions?: boolean
  // Audio props
  audioAvailable: boolean
  audioTooltip: string
  micMode: 'push_to_talk' | 'voice_activity'
  vadState: 'idle' | 'listening' | 'speech' | 'transcribing'
  recording: boolean
  transcribing: boolean
  onMicDown: (e?: React.SyntheticEvent) => void
  onMicUp: () => void
  onVadToggle: () => void
}

export function ChatInput({
  input,
  setInput,
  attachments,
  setAttachments,
  isReadOnly,
  sending,
  sendDisabled,
  onSend,
  onRequestSuggestions,
  requestingSuggestions,
  audioAvailable,
  audioTooltip,
  micMode,
  vadState,
  recording,
  transcribing,
  onMicDown,
  onMicUp,
  onVadToggle,
}: ChatInputProps) {
  const { t } = useTranslation()
  const fileInputRef = useRef<HTMLInputElement>(null)

  const handleFileSelect = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const files = e.target.files
      if (!files) return
      for (const file of Array.from(files)) {
        const reader = new FileReader()
        reader.onload = () => {
          setAttachments((prev) => [...prev, { name: file.name, type: file.type, data: reader.result as string }])
        }
        reader.readAsDataURL(file)
      }
      e.target.value = ''
    },
    [setAttachments],
  )

  const handleInputChange = useCallback(
    (e: React.ChangeEvent<HTMLTextAreaElement>) => {
      setInput(e.target.value)
      const el = e.target
      el.style.height = 'auto'
      el.style.height = `${Math.min(el.scrollHeight, 128)}px`
    },
    [setInput],
  )

  return (
    <>
      {/* Attachment chips */}
      {attachments.length > 0 && (
        <div className="flex flex-wrap gap-1.5 border-muted border-t bg-surface-base px-4 pt-2">
          {attachments.map((a) => (
            <div
              key={`${a.name}-${a.data.slice(-16)}`}
              className="flex items-center gap-1 rounded-full bg-surface-elevated px-2 py-0.5"
            >
              {a.type.startsWith('image/') && (
                <img src={a.data} alt={a.name} className={cn(iconSize.md, 'rounded object-cover')} />
              )}
              <span className={cn('max-w-[120px] truncate text-[10px]', colors.text.primary)}>{a.name}</span>
              <button
                type="button"
                onClick={() => setAttachments((prev) => prev.filter((x) => x !== a))}
                className="text-content-muted hover:text-semantic-error"
              >
                <X className={iconSize.xs} />
              </button>
            </div>
          ))}
        </div>
      )}
      {isReadOnly ? (
        <div
          className={cn(
            'flex items-center justify-center border-muted border-t bg-surface-base px-4 py-3 text-xs',
            colors.text.secondary,
          )}
        >
          {t('chat.read_only_notice', 'This session has ended. History is read-only.')}
        </div>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault()
            onSend()
          }}
          className={cn(
            'flex items-end gap-2 border-muted border-t bg-surface-base px-4 py-3',
            attachments.length > 0 && 'border-t-0 pt-1.5',
          )}
        >
          <input ref={fileInputRef} type="file" multiple onChange={handleFileSelect} className="hidden" />
          <Button variant="ghost" size="sm" type="button" onClick={() => fileInputRef.current?.click()}>
            <Paperclip className={iconSize.sm} />
          </Button>
          <textarea
            value={input}
            onChange={handleInputChange}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                onSend()
              }
            }}
            disabled={isReadOnly}
            placeholder={t('chat.input_placeholder')}
            rows={1}
            style={{ overflow: 'hidden' }}
            className={cn(
              'flex-1 resize-none border bg-surface-base px-3 py-2 text-sm placeholder-content-tertiary',
              radius.md,
              interaction.focusRing,
              interaction.interactive,
              colors.text.primary,
              'max-h-32 border-DEFAULT focus:border-brand-signal',
            )}
          />
          {micMode === 'voice_activity' ? (
            <button
              type="button"
              onClick={onVadToggle}
              disabled={isReadOnly || sending || !audioAvailable}
              className={cn(
                'flex items-center justify-center p-2',
                radius.md,
                interaction.interactive,
                interaction.focusRing,
                interaction.disabled,
                vadState === 'listening' && 'animate-pulse bg-status-connected text-content-inverse hover:opacity-90',
                vadState === 'speech' && 'animate-pulse bg-semantic-error text-content-inverse hover:opacity-90',
                vadState === 'transcribing' && 'bg-semantic-warning text-content-inverse hover:opacity-90',
                vadState === 'idle' && 'text-content-secondary hover:bg-surface-hover',
              )}
              title={
                vadState === 'idle'
                  ? t('chat.mic_vad_tooltip', 'Click to toggle listening')
                  : t('chat.mic_vad_stop', 'Click to stop listening')
              }
            >
              {vadState === 'transcribing' ? (
                <Loader2 className={cn(iconSize.sm, 'animate-spin')} />
              ) : (
                <Mic className={iconSize.sm} />
              )}
            </button>
          ) : (
            <button
              type="button"
              onMouseDown={onMicDown}
              onMouseUp={onMicUp}
              onMouseLeave={onMicUp}
              onTouchStart={onMicDown}
              onTouchEnd={onMicUp}
              onTouchCancel={onMicUp}
              disabled={isReadOnly || sending || transcribing || !audioAvailable}
              className={cn(
                'flex items-center justify-center p-2',
                radius.md,
                interaction.interactive,
                interaction.focusRing,
                interaction.disabled,
                transcribing && 'bg-semantic-warning text-content-inverse hover:opacity-90',
                recording && 'animate-pulse bg-semantic-error text-content-inverse hover:opacity-90',
                !recording && !transcribing && 'text-content-secondary hover:bg-surface-hover',
              )}
              title={audioTooltip}
            >
              {transcribing ? <Loader2 className={cn(iconSize.sm, 'animate-spin')} /> : <Mic className={iconSize.sm} />}
            </button>
          )}
          {onRequestSuggestions && (
            <Button
              variant="ghost"
              size="sm"
              type="button"
              disabled={sendDisabled || requestingSuggestions}
              onClick={onRequestSuggestions}
              title="Get AI suggestions"
              className="shrink-0"
            >
              {requestingSuggestions ? (
                <Loader2 className={cn(iconSize.sm, 'animate-spin')} />
              ) : (
                <Lightbulb className={iconSize.sm} />
              )}
            </Button>
          )}
          <Button variant="primary" size="sm" type="submit" disabled={sendDisabled}>
            <Send className={iconSize.sm} />
          </Button>
        </form>
      )}
    </>
  )
}
