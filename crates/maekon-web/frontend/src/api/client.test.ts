import { describe, expect, it } from 'vitest'

import { buildGuiSessionStreamCookie, guiSessionEventsUrl } from './client'

describe('GUI session event stream auth', () => {
  it('returns an EventSource URL without leaking the capability token', () => {
    const url = guiSessionEventsUrl('session-1', 'secret-token')

    expect(url).toBe('/api/automation/gui/sessions/session-1/events')
    expect(url).not.toContain('secret-token')
    expect(url).not.toContain('token=')
  })

  it('builds a path-scoped stream cookie with encoded token value', () => {
    const cookie = buildGuiSessionStreamCookie('session-1', ' secret:token ')

    expect(cookie).toBe(
      'maekon_gui_session_token=secret%3Atoken; Path=/api/automation/gui/sessions/session-1/events; SameSite=Strict',
    )
  })
})
