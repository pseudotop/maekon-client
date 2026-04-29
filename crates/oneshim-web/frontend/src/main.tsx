import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { ThemeProvider } from './contexts/ThemeContext'
import { installFrontendLogBridge } from './logging/frontendLogger'
import { AppBrowserRouter } from './router/future'
import './i18n' // i18n initialize
import './index.css'

installFrontendLogBridge('main')

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchInterval: false, // No global polling — set per-query where needed
      staleTime: 30_000, // 30s default stale time
    },
  },
})

// biome-ignore lint/style/noNonNullAssertion: root element is guaranteed to exist in index.html
ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <AppBrowserRouter>
        <ThemeProvider>
          <App />
        </ThemeProvider>
      </AppBrowserRouter>
    </QueryClientProvider>
  </React.StrictMode>,
)
