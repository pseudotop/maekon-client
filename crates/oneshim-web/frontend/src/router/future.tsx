import type { BrowserRouterProps, FutureConfig, MemoryRouterProps } from 'react-router-dom'
import { BrowserRouter, MemoryRouter } from 'react-router-dom'

export const appRouterFuture: Partial<FutureConfig> = {
  v7_startTransition: true,
  v7_relativeSplatPath: true,
}

export function AppBrowserRouter({ future, ...props }: BrowserRouterProps) {
  return <BrowserRouter {...props} future={{ ...appRouterFuture, ...future }} />
}

export function AppMemoryRouter({ future, ...props }: MemoryRouterProps) {
  return <MemoryRouter {...props} future={{ ...appRouterFuture, ...future }} />
}
