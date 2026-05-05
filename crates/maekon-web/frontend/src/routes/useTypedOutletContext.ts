import { useOutletContext } from 'react-router-dom'
import { OutletContextError } from './OutletContextError'

/**
 * Type-safe wrapper around useOutletContext.
 * Throws OutletContextError when the context is missing, which is caught
 * by RouteErrorBoundary and produces a clear developer-facing message.
 */
export function useTypedOutletContext<T>(routeName: string): T {
  const context = useOutletContext<T | undefined>()
  if (context === undefined || context === null) {
    throw new OutletContextError(routeName)
  }
  return context
}
