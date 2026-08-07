import { fireEvent, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import { makeDefaultFormData } from '../stories-utils'
import SandboxConfig from './SandboxConfig'

vi.mock('../../../utils/platform', () => ({ IS_WINDOWS: true }))

describe('SandboxConfig on Windows', () => {
  it('marks fail-closed profiles unavailable and explains the supported choice', () => {
    const formData = makeDefaultFormData()
    formData.sandbox.enabled = true
    formData.sandbox.profile = 'Standard'

    renderWithProviders(<SandboxConfig formData={formData} onSandboxChange={vi.fn()} />)

    expect(screen.getByRole('option', { name: 'Standard — Unavailable on Windows' })).toBeDisabled()
    expect(screen.getByRole('option', { name: 'Strict — Unavailable on Windows' })).toBeDisabled()
    expect(
      screen.getByText(
        'Windows currently runs only Permissive. Standard and Strict fail closed before execution because filesystem, syscall, and network containment are unavailable.',
      ),
    ).toBeInTheDocument()
  })

  it('lets an existing unsupported profile move to Permissive', () => {
    const formData = makeDefaultFormData()
    formData.sandbox.enabled = true
    formData.sandbox.profile = 'Standard'
    const onSandboxChange = vi.fn()

    renderWithProviders(<SandboxConfig formData={formData} onSandboxChange={onSandboxChange} />)

    fireEvent.change(screen.getByLabelText('Sandbox Profile'), { target: { value: 'Permissive' } })
    expect(onSandboxChange).toHaveBeenCalledWith('profile', 'Permissive')
  })
})
