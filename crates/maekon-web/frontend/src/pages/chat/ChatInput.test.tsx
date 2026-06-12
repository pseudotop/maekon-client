import { fireEvent, render, screen } from '@testing-library/react'
import type React from 'react'
import { I18nextProvider } from 'react-i18next'
import { describe, expect, it, vi } from 'vitest'
import i18n from '../../i18n'
import { ChatInput } from './ChatInput'

function renderChatInput(overrides: Partial<React.ComponentProps<typeof ChatInput>> = {}) {
  const props: React.ComponentProps<typeof ChatInput> = {
    input: '',
    setInput: vi.fn(),
    attachments: [],
    setAttachments: vi.fn(),
    isReadOnly: false,
    sending: false,
    sendDisabled: false,
    onSend: vi.fn(),
    onRequestSuggestions: vi.fn(),
    requestingSuggestions: false,
    audioAvailable: false,
    audioTooltip: 'Microphone unavailable',
    micMode: 'push_to_talk',
    vadState: 'idle',
    recording: false,
    transcribing: false,
    onMicDown: vi.fn(),
    onMicUp: vi.fn(),
    onVadToggle: vi.fn(),
    ...overrides,
  }

  render(
    <I18nextProvider i18n={i18n}>
      <ChatInput {...props} />
    </I18nextProvider>,
  )

  return props
}

describe('chat input suggestions', () => {
  it('fires the explicit AI suggestion request from the GUI button', () => {
    const onRequestSuggestions = vi.fn()
    renderChatInput({ onRequestSuggestions })

    fireEvent.click(screen.getByRole('button', { name: 'Get AI suggestions' }))

    expect(onRequestSuggestions).toHaveBeenCalledTimes(1)
  })

  it('disables the GUI suggestion request while a request is in flight', () => {
    renderChatInput({ requestingSuggestions: true })

    expect(screen.getByRole('button', { name: 'Get AI suggestions' })).toBeDisabled()
  })
})
