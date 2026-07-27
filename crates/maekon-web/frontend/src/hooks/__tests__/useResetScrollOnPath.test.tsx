import { fireEvent, render, screen } from '@testing-library/react'
import { useRef } from 'react'
import { MemoryRouter, useNavigate } from 'react-router-dom'
import { useResetScrollOnPath } from '../useResetScrollOnPath'

function Harness() {
  const scrollRef = useRef<HTMLDivElement>(null)
  const navigate = useNavigate()
  useResetScrollOnPath(scrollRef)

  return (
    <>
      <button type="button" onClick={() => navigate('/settings/audio')}>
        Open audio settings
      </button>
      <div ref={scrollRef} data-testid="main-scroll" />
    </>
  )
}

describe('useResetScrollOnPath', () => {
  it('resets the shell scroll container when the pathname changes', () => {
    render(
      <MemoryRouter initialEntries={['/settings/advanced']}>
        <Harness />
      </MemoryRouter>,
    )

    const container = screen.getByTestId('main-scroll')
    container.scrollTop = 640

    fireEvent.click(screen.getByRole('button', { name: 'Open audio settings' }))

    expect(container.scrollTop).toBe(0)
  })
})
