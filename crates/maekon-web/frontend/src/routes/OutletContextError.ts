export class OutletContextError extends Error {
  constructor(routeName: string) {
    super(`[${routeName}] Outlet context is missing. This component must render inside its parent Layout.`)
    this.name = 'OutletContextError'
  }
}
