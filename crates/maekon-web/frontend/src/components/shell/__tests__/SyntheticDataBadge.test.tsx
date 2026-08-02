/**
 * #9611 WD-02.3 — the persistent synthetic-data label.
 *
 * The requirement is not "the badge renders" but "the badge does not disappear".
 * So these tests exercise the transitions it must survive — a route change, a
 * modal, a thrown child — and the one it must not survive, sign-out.
 */

import { act, render, screen } from '@testing-library/react'
import { Component, type ReactNode, useState } from 'react'
import { afterEach, describe, expect, it } from 'vitest'
import { SyntheticDataBadge } from '../SyntheticDataBadge'
import {
  __resetSyntheticSessionForTests,
  clearSyntheticSession,
  markSyntheticSession,
  readSyntheticSession,
} from '../syntheticSessionSignal'

afterEach(() => {
  __resetSyntheticSessionForTests()
})

/** Minimal boundary so a thrown child does not take the whole tree down. */
class Boundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false }
  static getDerivedStateFromError() {
    return { failed: true }
  }
  render() {
    return this.state.failed ? <div data-testid="boundary-caught" /> : this.props.children
  }
}

function Exploding() {
  throw new Error('route blew up')
}

describe('SyntheticDataBadge (#9611)', () => {
  it('says nothing before any evidence arrives', () => {
    // Claiming "demo data" about a session that might be talking to a real
    // server is the opposite error, and just as wrong.
    render(<SyntheticDataBadge />)
    expect(screen.queryByTestId('synthetic-data-badge')).not.toBeInTheDocument()
  })

  it('appears once the server states synthetic provenance', () => {
    render(<SyntheticDataBadge />)
    act(() => markSyntheticSession({ synthetic: true, seedNamespaces: ['wd_brokerage'] }))
    expect(screen.getByTestId('synthetic-data-badge')).toBeInTheDocument()
  })

  it('carries meaning without colour and is exposed to assistive tech', () => {
    markSyntheticSession({ synthetic: true, seedNamespaces: [] })
    render(<SyntheticDataBadge />)

    const badge = screen.getByTestId('synthetic-data-badge')
    // A greyscale screenshot or a high-contrast theme must still read as "demo".
    expect(badge.textContent?.trim()).not.toBe('')
    expect(badge).toHaveAttribute('role', 'status')
    expect(badge.getAttribute('aria-label')).toBeTruthy()
  })

  it('survives a sibling route unmounting and remounting', () => {
    markSyntheticSession({ synthetic: true, seedNamespaces: [] })

    function Host() {
      const [route, setRoute] = useState('a')
      return (
        <>
          <SyntheticDataBadge />
          <button type="button" data-testid="nav" onClick={() => setRoute((r) => (r === 'a' ? 'b' : 'a'))}>
            nav
          </button>
          {route === 'a' ? <div data-testid="route-a" /> : <div data-testid="route-b" />}
        </>
      )
    }

    render(<Host />)
    expect(screen.getByTestId('synthetic-data-badge')).toBeInTheDocument()
    act(() => screen.getByTestId('nav').click())
    expect(screen.getByTestId('route-b')).toBeInTheDocument()
    expect(screen.getByTestId('synthetic-data-badge')).toBeInTheDocument()
  })

  it('survives a sibling route throwing into an error boundary', () => {
    // A route-level crash is exactly when a viewer most needs to know the data
    // on screen was never real.
    markSyntheticSession({ synthetic: true, seedNamespaces: [] })

    render(
      <>
        <SyntheticDataBadge />
        <Boundary>
          <Exploding />
        </Boundary>
      </>,
    )

    expect(screen.getByTestId('boundary-caught')).toBeInTheDocument()
    expect(screen.getByTestId('synthetic-data-badge')).toBeInTheDocument()
  })

  it('is not cleared by a later snapshot that omits the flag', () => {
    // An unavailable or partially-served home is not evidence that the session
    // stopped being a demo — treating it as such is the flicker this prevents.
    markSyntheticSession({ synthetic: true, seedNamespaces: ['wd_brokerage'] })
    render(<SyntheticDataBadge />)

    act(() => markSyntheticSession({ synthetic: false, seedNamespaces: [] }))
    expect(screen.getByTestId('synthetic-data-badge')).toBeInTheDocument()
    expect(readSyntheticSession()?.synthetic).toBe(true)
  })

  it('is cleared by sign-out so the next session re-earns the claim', () => {
    markSyntheticSession({ synthetic: true, seedNamespaces: [] })
    render(<SyntheticDataBadge />)
    expect(screen.getByTestId('synthetic-data-badge')).toBeInTheDocument()

    act(() => clearSyntheticSession())
    expect(screen.queryByTestId('synthetic-data-badge')).not.toBeInTheDocument()
    expect(readSyntheticSession()).toBeNull()
  })
})
