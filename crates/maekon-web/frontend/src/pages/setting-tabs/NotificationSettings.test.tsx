import { fireEvent, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import NotificationSettings from './NotificationSettings'
import { makeDefaultFormData } from './stories-utils'

describe('NotificationSettings', () => {
  it('offers a localized test notification action when notifications are enabled', () => {
    const onTestNotification = vi.fn()
    const formData = makeDefaultFormData()

    renderWithProviders(
      <NotificationSettings
        notification={formData.notification}
        onChange={vi.fn()}
        onTestNotification={onTestNotification}
        isTestNotificationPending={false}
      />,
    )

    fireEvent.click(screen.getByRole('button', { name: /send test notification/i }))

    expect(onTestNotification).toHaveBeenCalledTimes(1)
  })

  it('keeps the test notification action disabled when notifications are disabled', () => {
    const formData = makeDefaultFormData()

    renderWithProviders(
      <NotificationSettings
        notification={{ ...formData.notification, enabled: false }}
        onChange={vi.fn()}
        onTestNotification={vi.fn()}
        isTestNotificationPending={false}
      />,
    )

    expect(screen.getByRole('button', { name: /send test notification/i })).toBeDisabled()
  })
})
