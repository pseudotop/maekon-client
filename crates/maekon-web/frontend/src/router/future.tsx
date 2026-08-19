import type { BrowserRouterProps, MemoryRouterProps } from 'react-router-dom'
import { BrowserRouter, MemoryRouter } from 'react-router-dom'

export function AppBrowserRouter(props: BrowserRouterProps) {
  return <BrowserRouter {...props} />
}

export function AppMemoryRouter(props: MemoryRouterProps) {
  return <MemoryRouter {...props} />
}
