import type { ReactNode } from 'react'
import ErrorBoundary from '../components/ErrorBoundary'
import { SettingsFormProvider } from '../pages/settings/SettingsFormContext'
import RouteRenderer from './RouteRenderer'

/**
 * Keep route-scoped drafts alive while users move between top-level screens.
 *
 * SettingsLayout still owns the settings UI and its route-local error
 * boundary, but the form provider must outlive SettingsLayout itself. If the
 * provider is mounted inside `/settings`, leaving that route silently drops
 * unsaved edits before the user can save or revert them.
 */
export function PersistentRouteScope({ children }: { children: ReactNode }) {
  return (
    <SettingsFormProvider>
      <ErrorBoundary>{children}</ErrorBoundary>
    </SettingsFormProvider>
  )
}

export default function PersistentRouteRenderer() {
  return (
    <PersistentRouteScope>
      <RouteRenderer />
    </PersistentRouteScope>
  )
}
