import { fireEvent, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import en from '../../i18n/locales/en.json'
import { MessageBubble } from './MessageBubble'
import type { ChatMessage } from './types'

function failedReply(error: NonNullable<ChatMessage['error']>): ChatMessage {
  return { role: 'assistant', content: '', timestamp: '2026-11-10T09:00:00Z', error }
}

describe('MessageBubble stream failure card (#13737)', () => {
  it('leads with a localized sentence and keeps the provider code as support detail', () => {
    const onRetry = vi.fn()
    renderWithProviders(
      <MessageBubble
        msg={failedReply({ code: 'turn_timeout', message: 'turn exceeded 120s without output', retryable: true })}
        onRetry={onRetry}
      />,
    )

    expect(screen.getByText(en.chat.response_failed)).toBeInTheDocument()
    expect(screen.queryByText('turn_timeout')).toBeNull()
    expect(screen.getByTestId('support-details-text')).toHaveTextContent(
      'turn_timeout: turn exceeded 120s without output',
    )

    fireEvent.click(screen.getByRole('button', { name: en.chat.retry }))
    expect(onRetry).toHaveBeenCalledTimes(1)
  })

  it('offers no retry for a failure the stream marked final', () => {
    renderWithProviders(
      <MessageBubble msg={failedReply({ code: 'rate_limit', message: '', retryable: false })} onRetry={vi.fn()} />,
    )

    expect(screen.queryByRole('button', { name: en.chat.retry })).toBeNull()
    expect(screen.getByTestId('support-details-text')).toHaveTextContent('rate_limit')
  })
})
