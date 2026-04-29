import { render } from '@testing-library/react'
import { Outlet, Route, Routes } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { AppMemoryRouter } from '../../router/future'
import { OutletContextError } from '../OutletContextError'
import { useTypedOutletContext } from '../useTypedOutletContext'

interface TestCtx {
  value: string
}

function Child() {
  const ctx = useTypedOutletContext<TestCtx>('TestRoute')
  return <div>{ctx.value}</div>
}

function ParentWithContext() {
  return <Outlet context={{ value: 'hello' } satisfies TestCtx} />
}

function ParentNoContext() {
  // Intentionally omit context prop — useTypedOutletContext should throw.
  return <Outlet />
}

describe('useTypedOutletContext', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('returns context when provided by the parent Outlet', () => {
    const { getByText } = render(
      <AppMemoryRouter initialEntries={['/parent/child']}>
        <Routes>
          <Route path="parent" element={<ParentWithContext />}>
            <Route path="child" element={<Child />} />
          </Route>
        </Routes>
      </AppMemoryRouter>,
    )
    expect(getByText('hello')).toBeInTheDocument()
  })

  it('throws OutletContextError when context is missing', () => {
    // Suppress React's error boundary noise for cleaner test output.
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})

    expect(() =>
      render(
        <AppMemoryRouter initialEntries={['/parent/child']}>
          <Routes>
            <Route path="parent" element={<ParentNoContext />}>
              <Route path="child" element={<Child />} />
            </Route>
          </Routes>
        </AppMemoryRouter>,
      ),
    ).toThrow(OutletContextError)

    consoleError.mockRestore()
  })

  it('OutletContextError message includes the route name for debuggability', () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})

    try {
      render(
        <AppMemoryRouter initialEntries={['/parent/child']}>
          <Routes>
            <Route path="parent" element={<ParentNoContext />}>
              <Route path="child" element={<Child />} />
            </Route>
          </Routes>
        </AppMemoryRouter>,
      )
      expect.fail('Expected render to throw')
    } catch (err) {
      expect(err).toBeInstanceOf(OutletContextError)
      expect((err as Error).message).toContain('TestRoute')
    }

    consoleError.mockRestore()
  })
})
