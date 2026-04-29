import { createContext, type ReactNode, useContext, useMemo } from 'react'

interface ShellLayoutContextValue {
  sidebarCollapsed: boolean
}

const ShellLayoutContext = createContext<ShellLayoutContextValue>({
  sidebarCollapsed: false,
})

interface ShellLayoutProviderProps {
  children: ReactNode
  sidebarCollapsed: boolean
}

export function ShellLayoutProvider({ children, sidebarCollapsed }: ShellLayoutProviderProps) {
  const value = useMemo(() => ({ sidebarCollapsed }), [sidebarCollapsed])
  return <ShellLayoutContext.Provider value={value}>{children}</ShellLayoutContext.Provider>
}

export function useShellLayoutContext() {
  return useContext(ShellLayoutContext)
}
