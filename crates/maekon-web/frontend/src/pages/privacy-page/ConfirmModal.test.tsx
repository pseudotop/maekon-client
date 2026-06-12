/**
 * ConfirmModal — erasure export-disclosure (#4478 G5).
 *
 * The "Delete all data" (GDPR right-to-erasure) confirmation must disclose that
 * files the user already exported or backed up to disk survive the wipe (they
 * live outside the app). The modal renders this as a distinct warning callout.
 */

import { render, screen } from '@testing-library/react'
import { I18nextProvider } from 'react-i18next'
import { describe, expect, it, vi } from 'vitest'
import i18n from '../../i18n'
import { ConfirmModal } from './PrivacyLayout'

function renderModal(note?: string) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ConfirmModal
        isOpen
        title="Delete All Data"
        message="Are you sure?"
        confirmText="Delete"
        isDangerous
        note={note}
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />
    </I18nextProvider>,
  )
}

describe('ConfirmModal — erasure export disclosure (#4478 G5)', () => {
  it('renders the disclosure note as a distinct warning alert when provided', () => {
    const note = 'Exported files saved to disk survive this action.'
    renderModal(note)
    // role="alert" is what Alert variant="warning" emits (distinct from the
    // surrounding role="alertdialog"), so the disclosure is announced.
    const alert = screen.getByRole('alert')
    expect(alert).toHaveTextContent(note)
  })

  it('omits the disclosure (no warning alert) when no note is provided', () => {
    renderModal(undefined)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('deleteAllExportNote translation resolves and names exported/backup files', () => {
    const note = i18n.t('privacy.deleteAllExportNote')
    // Key resolves to a real string, not the bare key.
    expect(note).not.toBe('privacy.deleteAllExportNote')
    // "JSON/CSV" is the shared token present in every locale's translation.
    expect(note).toContain('JSON/CSV')
  })
})
