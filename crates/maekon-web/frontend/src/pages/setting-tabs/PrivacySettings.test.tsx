import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { PrivacySettings as PrivacySettingsType } from '../../api/contracts'
import PrivacySettings from './PrivacySettings'

// Return the key verbatim so assertions can match on the i18n key.
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (k: string) => k }),
}))

/**
 * #9146: the PII level `<select>` option values are a CONTRACT with the
 * backend settings assembler's canonical serde tokens ("Off"/"Basic"/
 * "Standard"/"Strict" — locked backend-side by
 * `settings_token_matches_serde_token` and the handler round-trip test).
 * The old lowercase Display-based response ("strict") matched no option, so
 * the browser silently fell back to the first option ("Off") right after a
 * successful save. These tests pin both sides of that contract in the UI.
 */
const CANONICAL_PII_TOKENS = ['Off', 'Basic', 'Standard', 'Strict']

function privacyFixture(pii: string): PrivacySettingsType {
  return {
    excluded_apps: [],
    excluded_app_patterns: [],
    excluded_title_patterns: [],
    auto_exclude_sensitive: false,
    pii_filter_level: pii,
  }
}

describe('PrivacySettings PII level select (#9146)', () => {
  it('option values equal the canonical backend tokens, in order', () => {
    render(<PrivacySettings privacy={privacyFixture('Standard')} onChange={() => {}} />)
    const select = screen.getByLabelText('settings.piiLevel') as HTMLSelectElement
    const optionValues = Array.from(select.options).map((o) => o.value)
    expect(optionValues).toEqual(CANONICAL_PII_TOKENS)
  })

  it('renders a canonical "Strict" save-response value as Strict (not the first option)', () => {
    // Simulates the post-save reconciliation: formData adopts the save
    // response verbatim (useSettingsForm saveMutation.onSuccess), so the
    // select must render the canonical token it carries.
    render(<PrivacySettings privacy={privacyFixture('Strict')} onChange={() => {}} />)
    const select = screen.getByLabelText('settings.piiLevel') as HTMLSelectElement
    expect(select.value).toBe('Strict')
  })

  it('emits the canonical token on change (what the save request sends)', () => {
    const onChange = vi.fn()
    render(<PrivacySettings privacy={privacyFixture('Off')} onChange={onChange} />)
    const select = screen.getByLabelText('settings.piiLevel') as HTMLSelectElement
    fireEvent.change(select, { target: { value: 'Strict' } })
    expect(onChange).toHaveBeenCalledWith('pii_filter_level', 'Strict')
  })
})
