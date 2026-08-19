import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import type { Tag } from '../api/contracts'
import { TagManagementCard } from './TagManagementCard'

const fetchTags = vi.fn()
const updateTag = vi.fn()
const deleteTag = vi.fn()

// Spread the real module so non-mocked exports (TAGS_QUERY_KEY) stay intact —
// a bare factory silently drops them and the component throws on import.
vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  fetchTags: (...args: unknown[]) => fetchTags(...args),
  updateTag: (...args: unknown[]) => updateTag(...args),
  deleteTag: (...args: unknown[]) => deleteTag(...args),
}))

const TAGS: Tag[] = [
  { id: 1, name: 'deep-work', color: '#3b82f6', created_at: '2026-07-01T00:00:00Z' },
  { id: 2, name: 'meetings', color: '#ef4444', created_at: '2026-07-02T00:00:00Z' },
]

describe('TagManagementCard', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    fetchTags.mockResolvedValue(TAGS)
    updateTag.mockResolvedValue({ ...TAGS[0], name: 'focus' })
    deleteTag.mockResolvedValue(undefined)
  })

  it('renames a tag through the update API and keeps its color', async () => {
    renderWithProviders(<TagManagementCard />)
    await screen.findByText('deep-work')

    fireEvent.click(screen.getAllByRole('button', { name: 'Rename' })[0])
    const input = await screen.findByLabelText('Tag name')
    expect(input).toHaveValue('deep-work')

    fireEvent.change(input, { target: { value: '  focus  ' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await waitFor(() => expect(updateTag).toHaveBeenCalledWith(1, { name: 'focus', color: '#3b82f6' }))
  })

  it('blocks a rename that collides with another tag name', async () => {
    renderWithProviders(<TagManagementCard />)
    await screen.findByText('deep-work')

    fireEvent.click(screen.getAllByRole('button', { name: 'Rename' })[0])
    fireEvent.change(await screen.findByLabelText('Tag name'), { target: { value: 'meetings' } })

    expect(await screen.findByText('Another tag already uses this name.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled()
    expect(updateTag).not.toHaveBeenCalled()
  })

  it('deletes only after the confirmation dialog is accepted', async () => {
    renderWithProviders(<TagManagementCard />)
    await screen.findByText('meetings')

    fireEvent.click(screen.getAllByRole('button', { name: 'Delete' })[1])
    expect(deleteTag).not.toHaveBeenCalled()

    const dialog = await screen.findByRole('dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }))

    await waitFor(() => expect(deleteTag).toHaveBeenCalledWith(2))
  })

  it('surfaces a failed delete instead of silently closing', async () => {
    deleteTag.mockRejectedValue(new Error('tag is still referenced'))
    renderWithProviders(<TagManagementCard />)
    await screen.findByText('meetings')

    fireEvent.click(screen.getAllByRole('button', { name: 'Delete' })[1])
    const dialog = await screen.findByRole('dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }))

    // The message belongs inside the dialog the user is still looking at.
    expect(await within(dialog).findByText('tag is still referenced')).toBeInTheDocument()
  })

  it('refreshes the shared tag list after a rename', async () => {
    // The card's headline claim: other surfaces read the same ['tags'] key, so
    // invalidation is what makes a rename visible on the timeline and search.
    renderWithProviders(<TagManagementCard />)
    await screen.findByText('deep-work')
    expect(fetchTags).toHaveBeenCalledTimes(1)

    fireEvent.click(screen.getAllByRole('button', { name: 'Rename' })[0])
    fireEvent.change(await screen.findByLabelText('Tag name'), { target: { value: 'focus' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await waitFor(() => expect(fetchTags).toHaveBeenCalledTimes(2))
  })

  it('discards a failed mutation error when the dialog is dismissed', async () => {
    // Closing the dialog used to leave the error set, so it re-rendered as an
    // undismissable banner on the card body.
    updateTag.mockRejectedValue(new Error('boom'))
    renderWithProviders(<TagManagementCard />)
    await screen.findByText('deep-work')

    fireEvent.click(screen.getAllByRole('button', { name: 'Rename' })[0])
    fireEvent.change(await screen.findByLabelText('Tag name'), { target: { value: 'focus' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    expect(await screen.findByText('boom')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(screen.queryByText('boom')).not.toBeInTheDocument()
  })

  it('discards a failed delete error when its dialog is dismissed', async () => {
    // Symmetric to the rename case above — closeDeleting must clear the error
    // on the same paths.
    deleteTag.mockRejectedValue(new Error('nope'))
    renderWithProviders(<TagManagementCard />)
    await screen.findByText('meetings')

    fireEvent.click(screen.getAllByRole('button', { name: 'Delete' })[1])
    const dialog = await screen.findByRole('dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }))
    expect(await within(dialog).findByText('nope')).toBeInTheDocument()

    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))

    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(screen.queryByText('nope')).not.toBeInTheDocument()
  })

  it('allows renaming a tag whose name differs only by case', async () => {
    // SQLite UNIQUE on TEXT is BINARY, so "Meetings" and "meetings" coexist. A
    // case-insensitive guard would disable Save here and block even a colour
    // change on that tag.
    renderWithProviders(<TagManagementCard />)
    await screen.findByText('deep-work')

    fireEvent.click(screen.getAllByRole('button', { name: 'Rename' })[0])
    fireEvent.change(await screen.findByLabelText('Tag name'), { target: { value: 'Meetings' } })

    expect(screen.queryByText('Another tag already uses this name.')).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled()
  })
})
