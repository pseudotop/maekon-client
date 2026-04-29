import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type RenderOptions, render } from '@testing-library/react'
import type { ReactElement } from 'react'
import { I18nextProvider } from 'react-i18next'
import type { MemoryRouterProps } from 'react-router-dom'
import { ThemeProvider } from '../../contexts/ThemeContext'
import i18n from '../../i18n'
import { AppMemoryRouter } from '../../router/future'

interface ProvidersProps {
  children: React.ReactNode
  routerProps?: MemoryRouterProps
}

function createTestQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  })
}

function AllProviders({ children, routerProps }: ProvidersProps) {
  const queryClient = createTestQueryClient()
  return (
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={queryClient}>
        <ThemeProvider>
          <AppMemoryRouter {...routerProps}>{children}</AppMemoryRouter>
        </ThemeProvider>
      </QueryClientProvider>
    </I18nextProvider>
  )
}

export function renderWithProviders(
  ui: ReactElement,
  options?: Omit<RenderOptions, 'wrapper'> & { routerProps?: MemoryRouterProps },
) {
  const { routerProps, ...renderOptions } = options ?? {}
  return render(ui, {
    wrapper: ({ children }) => <AllProviders routerProps={routerProps}>{children}</AllProviders>,
    ...renderOptions,
  })
}
