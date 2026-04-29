import { Suspense } from 'react'
import { Navigate, Route, Routes } from 'react-router-dom'
import { Spinner } from '../components/ui'
import { RouteErrorBoundary } from './RouteErrorBoundary'
import { type RouteNode, routeTree } from './route-tree'

/**
 * Render a route node's component, optionally wrapped in RouteErrorBoundary.
 *
 * Extracted into a function so biome's `useJsxKeyInIterable` lint doesn't
 * flag the JSX as if it were a direct list item. The result is used inside
 * a `<Route element={...}>` prop which carries its own key on the Route.
 */
function renderRouteElement(node: RouteNode) {
  const Component = node.component
  if (node.selfWraps) {
    return <Component />
  }
  return (
    <RouteErrorBoundary route={node.path}>
      <Component />
    </RouteErrorBoundary>
  )
}

// Validate routeTree once at module load (dev only).
// Catches misconfiguration before any render — fail fast.
if (import.meta.env.DEV) {
  for (const node of routeTree) {
    if (node.children && node.defaultChild) {
      const match = node.children.some((c) => c.path === node.defaultChild)
      if (!match) {
        throw new Error(
          `[RouteRenderer] ${node.path} defaultChild="${node.defaultChild}" ` +
            `not found in children: [${node.children.map((c) => c.path).join(', ')}]`,
        )
      }
    }
    if (node.children && !node.defaultChild) {
      throw new Error(`[RouteRenderer] ${node.path} has children but no defaultChild`)
    }
  }
}

// Sort once at module load: leaf routes first, parent routes after, root "/" last.
// Prevents "/" catch-all from consuming other routes.
const sortedRouteTree = [...routeTree].sort((a, b) => {
  if (a.path === '/') return 1
  if (b.path === '/') return -1
  if (a.children && !b.children) return 1
  if (!a.children && b.children) return -1
  return 0
})

export default function RouteRenderer() {
  return (
    <Suspense
      fallback={
        <div className="flex min-h-full items-center justify-center">
          <Spinner size="lg" />
        </div>
      }
    >
      <Routes>
        {sortedRouteTree.map((node) =>
          // Parent layouts get exactly ONE outer RouteErrorBoundary so a
          // crash anywhere in the layout (header, useQuery, Outlet child)
          // is isolated per-route. Earlier revisions also wrapped the
          // <Outlet> inside each Layout with a second boundary, but two
          // boundaries on the same route key produced duplicate
          // recoverySignals subscribers — halving the escalation threshold
          // from 3 to 2 (commit e5bab2e1). Do not re-introduce the inner
          // wrapping without also re-keying the recovery channel.
          //
          // Exception: nodes with `selfWraps: true` are responsible for
          // placing their own boundary. Used when a stateful Provider must
          // live ABOVE the boundary so its state survives recovery reset
          // (e.g., SettingsFormProvider preserves unsaved form edits).
          node.children ? (
            // For the root node ("/") the splat path is just "/*" — naively
            // concatenating would produce "//*", which react-router treats as
            // a literal double-slash and silently breaks child path resolution
            // (e.g. /monitoring would fall through to the index Navigate
            // instead of mounting MonitoringSection). Caught after sub-route
            // direct-navigation tests started failing — see e2e/dashboard.spec.
            <Route
              key={node.path}
              path={node.path === '/' ? '/*' : `${node.path}/*`}
              element={renderRouteElement(node)}
            >
              {node.defaultChild && <Route index element={<Navigate to={node.defaultChild} replace />} />}
              {node.children.map((child) => {
                const ChildComponent = child.component
                return <Route key={child.path} path={child.path} element={<ChildComponent />} />
              })}
            </Route>
          ) : (
            <Route key={node.path} path={node.path} element={renderRouteElement(node)} />
          ),
        )}
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </Suspense>
  )
}
