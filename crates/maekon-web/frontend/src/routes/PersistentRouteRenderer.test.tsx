import { fireEvent, render, screen } from '@testing-library/react'
import type { ChangeEvent, ReactNode } from 'react'
import { MemoryRouter, useLocation, useNavigate } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'
import { useSettingsFormContext } from '../pages/settings/SettingsFormContext'
import { PersistentRouteScope } from './PersistentRouteRenderer'

vi.mock('./RouteRenderer', () => ({ default: () => null }))
vi.mock('../components/ErrorBoundary', () => ({
  default: ({ children }: { children: ReactNode }) => children,
}))

vi.mock('../pages/settings/SettingsFormContext', async () => {
  const React = await import('react')
  const DraftContext = React.createContext<{
    form: {
      formData: { web_port: number }
      hasUnsavedChanges: boolean
      setWebPort: (port: number) => void
    }
  } | null>(null)

  return {
    SettingsFormProvider({ children }: { children: ReactNode }) {
      const [webPort, setWebPort] = React.useState(10090)
      return (
        <DraftContext.Provider
          value={{
            form: {
              formData: { web_port: webPort },
              hasUnsavedChanges: webPort !== 10090,
              setWebPort,
            },
          }}
        >
          {children}
        </DraftContext.Provider>
      )
    },
    useSettingsFormContext() {
      const context = React.useContext(DraftContext)
      if (!context) throw new Error('missing test settings context')
      return context
    },
  }
})

function RouteFixture() {
  const location = useLocation()
  const navigate = useNavigate()
  const { form } = useSettingsFormContext() as unknown as {
    form: {
      formData: { web_port: number }
      hasUnsavedChanges: boolean
      setWebPort: (port: number) => void
    }
  }

  if (location.pathname === '/support') {
    return (
      <button type="button" onClick={() => navigate('/settings/general')}>
        Return to settings
      </button>
    )
  }

  const handleChange = (event: ChangeEvent<HTMLInputElement>) => {
    form.setWebPort(Number(event.target.value))
  }

  return (
    <div>
      <input aria-label="Web dashboard port" value={form.formData.web_port} onChange={handleChange} />
      {form.hasUnsavedChanges && <div>Unsaved changes</div>}
      <button type="button" onClick={() => navigate('/support')}>
        Open support
      </button>
    </div>
  )
}

describe('PersistentRouteScope', () => {
  it('preserves an unsaved settings draft across top-level route navigation', () => {
    render(
      <MemoryRouter
        initialEntries={['/settings/general']}
        future={{ v7_startTransition: true, v7_relativeSplatPath: true }}
      >
        <PersistentRouteScope>
          <RouteFixture />
        </PersistentRouteScope>
      </MemoryRouter>,
    )

    fireEvent.change(screen.getByLabelText('Web dashboard port'), { target: { value: '10091' } })
    expect(screen.getByText('Unsaved changes')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Open support' }))
    fireEvent.click(screen.getByRole('button', { name: 'Return to settings' }))

    expect(screen.getByLabelText('Web dashboard port')).toHaveValue('10091')
    expect(screen.getByText('Unsaved changes')).toBeInTheDocument()
  })
})
