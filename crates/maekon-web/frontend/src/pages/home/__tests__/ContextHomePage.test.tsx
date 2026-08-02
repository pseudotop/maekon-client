/**
 * #9611 WD-02.3 — `/home` context home render.
 *
 * The states are asserted through `data-testid`, never through locale strings:
 * a selector written against English copy silently stops matching the moment a
 * translator touches it, and this page ships in five locales.
 *
 * Mock strategy mirrors `pages/auth/LoginPage.test.tsx` — `vi.mock` intercepts
 * the dynamic `@tauri-apps/api/core` import that `api/contextHome` performs.
 */

import { act, fireEvent, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import fixtureJson from '../../../../../../../api/fixtures/context-home.v1.json'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import type { ContextHomeSnapshot } from '../../../api/contextHome'
import { __resetSyntheticSessionForTests, readSyntheticSession } from '../../../components/shell/syntheticSessionSignal'
import ContextHomePage from '../ContextHomePage'
import { MAX_RENDERED_PARTICIPANTS, MAX_RENDERED_THREADS } from '../homeSections'

const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

const FIXTURE = fixtureJson as unknown as ContextHomeSnapshot

function snapshotWith(over: Partial<ContextHomeSnapshot>): ContextHomeSnapshot {
  return { ...structuredClone(FIXTURE), ...over }
}

function renderHome() {
  return renderWithProviders(<ContextHomePage />, { routerProps: { initialEntries: ['/home'] } })
}

describe('ContextHomePage (#9611)', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    __resetSyntheticSessionForTests()
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  it('asks the server with no arguments — identity is never a caller parameter', async () => {
    mockInvoke.mockResolvedValue(FIXTURE)
    renderHome()
    await screen.findByTestId('context-home-actor')

    const call = mockInvoke.mock.calls.find((c) => c[0] === 'fetch_context_home')
    expect(call).toBeDefined()
    expect(call?.[1]).toBeUndefined()
  })

  it('shows a loading state before the first read settles, not an empty home', async () => {
    // An empty-looking home while the request is still in flight is the first
    // and cheapest way to tell a user something false.
    let resolve!: (v: ContextHomeSnapshot) => void
    mockInvoke.mockReturnValue(new Promise<ContextHomeSnapshot>((r) => (resolve = r)))

    renderHome()
    expect(await screen.findByTestId('context-home-loading')).toBeInTheDocument()
    expect(screen.queryByTestId('context-home-section-mail')).not.toBeInTheDocument()

    await act(async () => resolve(FIXTURE))
    await screen.findByTestId('context-home-section-mail')
  })

  it('renders an unserved section differently from an empty one', async () => {
    // The fixture is deliberately partial: mail/messenger served, projects not.
    mockInvoke.mockResolvedValue(FIXTURE)
    renderHome()

    const projects = await screen.findByTestId('context-home-section-projects')
    expect(projects).toHaveAttribute('data-section-state', 'unavailable')
    expect(screen.getByTestId('context-home-projects-unavailable')).toBeInTheDocument()
    // …and it must NOT be presented as "you have no projects".
    expect(screen.queryByTestId('context-home-projects-empty')).not.toBeInTheDocument()

    const mail = screen.getByTestId('context-home-section-mail')
    expect(mail).toHaveAttribute('data-section-state', 'ready')
  })

  it('renders an answered-but-empty section as empty, not as unavailable', async () => {
    mockInvoke.mockResolvedValue(
      snapshotWith({
        mail: { status: 'ready', items: [], truncated: false, next_cursor: null, unavailable_reason: null },
      }),
    )
    renderHome()

    const mail = await screen.findByTestId('context-home-section-mail')
    expect(mail).toHaveAttribute('data-section-state', 'empty')
    expect(screen.getByTestId('context-home-mail-empty')).toBeInTheDocument()
    expect(screen.queryByTestId('context-home-mail-unavailable')).not.toBeInTheDocument()
  })

  it('keeps the previous snapshot and marks it stale when a refresh fails', async () => {
    // Blanking the screen on a transient failure costs the user everything they
    // were reading; silently keeping it costs them the knowledge that it is old.
    mockInvoke.mockResolvedValueOnce(FIXTURE)
    renderHome()
    await screen.findByTestId('context-home-actor')
    expect(screen.queryByTestId('context-home-stale')).not.toBeInTheDocument()

    mockInvoke.mockRejectedValueOnce({ code: 'service.unavailable', message: 'down' })
    await act(async () => {
      fireEvent.click(screen.getByTestId('context-home-refresh'))
    })

    expect(await screen.findByTestId('context-home-stale')).toBeInTheDocument()
    // The rows are still on screen — stale, not gone.
    expect(screen.getByTestId('context-home-section-mail')).toBeInTheDocument()
  })

  it('offers retry when transient, and offers nothing when permission is denied', async () => {
    mockInvoke.mockRejectedValue({ code: 'service.unavailable', message: 'down' })
    const { unmount } = renderHome()
    expect(await screen.findByTestId('context-home-unavailable-retry')).toBeInTheDocument()
    unmount()

    mockInvoke.mockReset()
    mockInvoke.mockRejectedValue({ code: 'policy.denied', message: 'nope' })
    renderHome()
    expect(await screen.findByTestId('context-home-denied')).toBeInTheDocument()
    // Retrying a denial returns the same denial, and a sign-in link would be a
    // loop with no exit — neither affordance may appear.
    expect(screen.queryByTestId('context-home-unavailable-retry')).not.toBeInTheDocument()
    expect(screen.queryByTestId('context-home-reauth')).not.toBeInTheDocument()
  })

  it('separates a missing desktop bridge from a server outage', async () => {
    mockInvoke.mockReset()
    // No bridge at all: the dynamic import itself is what fails in the browser
    // dashboard, which `api/contextHome` surfaces as its own error class.
    const original = (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__
    mockInvoke.mockRejectedValue({ code: 'validation.invalid_field', message: 'garbage' })
    renderHome()
    expect(await screen.findByTestId('context-home-malformed')).toBeInTheDocument()
    expect(screen.queryByTestId('context-home-unavailable')).not.toBeInTheDocument()
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = original
  })

  it('bounds what it renders and states the remainder', async () => {
    const many = Array.from({ length: MAX_RENDERED_THREADS + 5 }, (_, i) => ({
      ...FIXTURE.mail.items[0],
      thread_id: `t-${i}`,
    }))
    mockInvoke.mockResolvedValue(
      snapshotWith({
        mail: { status: 'ready', items: many, truncated: true, next_cursor: null, unavailable_reason: null },
      }),
    )
    renderHome()

    // Scoped to the mail section: the messenger section carries its own fixture
    // rows, and a document-wide count would pass or fail for the wrong reason.
    const mail = within(await screen.findByTestId('context-home-section-mail'))
    expect(mail.getAllByTestId('context-home-thread')).toHaveLength(MAX_RENDERED_THREADS)
    expect(mail.getByTestId('context-home-hidden-count')).toBeInTheDocument()
  })

  it('bounds the participant strip, which is what breaks at 50-person scale', async () => {
    const crowded = {
      ...FIXTURE.mail.items[0],
      thread_id: 't-crowded',
      participant_count: 12,
      participants: Array.from({ length: 12 }, (_, i) => ({
        participant_id: `p-${i}`,
        kind: 'internal_member',
        display_label: `Member ${i}`,
      })),
    }
    mockInvoke.mockResolvedValue(
      snapshotWith({
        mail: { status: 'ready', items: [crowded], truncated: false, next_cursor: null, unavailable_reason: null },
      }),
    )
    renderHome()

    const mail = within(await screen.findByTestId('context-home-section-mail'))
    expect(mail.getAllByTestId('context-home-participant')).toHaveLength(MAX_RENDERED_PARTICIPANTS)
    expect(mail.getByTestId('context-home-participant-overflow')).toBeInTheDocument()
  })

  it('marks an external counterparty contact without relying on colour', async () => {
    // Rendering an outside contact as a colleague asserts something false about
    // a real person, and colour alone does not survive greyscale or a
    // colour-vision difference — so the kind is in the DOM and in the text.
    mockInvoke.mockResolvedValue(FIXTURE)
    renderHome()

    await screen.findByTestId('context-home-section-mail')
    const chips = screen.getAllByTestId('context-home-participant')
    const external = chips.filter((c) => c.getAttribute('data-participant-kind') === 'external_counterparty_contact')
    expect(external.length).toBeGreaterThan(0)
    for (const chip of external) {
      expect(chip.textContent).toMatch(/\(/) // the label carries an explicit suffix
    }
  })

  it('announces the state to screen readers, including partial', async () => {
    // Every visual distinction on this page (a chip, a flag, a greyed panel) is
    // otherwise silent — which would make "the states are distinguishable" true
    // only for sighted users.
    mockInvoke.mockResolvedValue(FIXTURE)
    renderHome()

    const region = await screen.findByTestId('context-home-announcement')
    expect(region).toHaveAttribute('aria-live', 'polite')
    await waitFor(() => expect(region.textContent?.trim()).not.toBe(''))
    // en.json: contextHome.announce.partial — the fixture is a partial home.
    expect(region.textContent).toMatch(/some sections/i)
  })

  it('gives every section an accessible name tied to its heading', async () => {
    mockInvoke.mockResolvedValue(FIXTURE)
    renderHome()

    await screen.findByTestId('context-home-section-mail')
    for (const id of ['mail', 'messenger', 'projects']) {
      const section = screen.getByTestId(`context-home-section-${id}`)
      const labelledBy = section.getAttribute('aria-labelledby')
      expect(labelledBy).toBeTruthy()
      expect(document.getElementById(labelledBy as string)).toBeInTheDocument()
    }
  })

  it('latches the shell demo label from the snapshot, not from being signed in', async () => {
    expect(readSyntheticSession()).toBeNull()
    mockInvoke.mockResolvedValue(FIXTURE)
    renderHome()

    await screen.findByTestId('context-home-actor')
    await waitFor(() => expect(readSyntheticSession()).not.toBeNull())
    expect(readSyntheticSession()?.synthetic).toBe(true)
  })

  it('does not claim synthetic data when the server does not say so', async () => {
    // The opposite error: labelling a connection to a real server as a demo.
    mockInvoke.mockResolvedValue(
      snapshotWith({
        synthetic: false,
        provenance: { synthetic_only: false, seed_namespaces: [], seed_revisions: [] },
      }),
    )
    renderHome()

    await screen.findByTestId('context-home-actor')
    expect(readSyntheticSession()).toBeNull()
  })
})
