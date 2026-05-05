import type { Decorator } from '@storybook/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactNode, useState } from 'react'
import { ActivityBar, SidePanel, TitleBar } from '../components/shell'
import { Card } from '../components/ui'
import { ShellLayoutProvider } from '../contexts/ShellLayoutContext'
import { AppMemoryRouter } from '../router/future'
import { layout } from '../styles/tokens'
import { cn } from '../utils/cn'

type SeedQueryFn = (client: QueryClient) => void

interface StoryProvidersProps {
  children: ReactNode
  initialEntries?: string[]
  seedQuery?: SeedQueryFn
  withShellLayout?: boolean
  sidebarCollapsed?: boolean
}

interface ShellStoryFrameProps {
  children: ReactNode
  route?: string
  sidebarCollapsed?: boolean
  sidebarWidth?: number
  contentClassName?: string
}

export const lightThemeGlobals = { theme: 'light' } as const
export const darkThemeGlobals = { theme: 'dark' } as const
export const reviewStoryParameters = { layout: 'fullscreen' } as const

export function createStoryQueryClient(seedQuery?: SeedQueryFn) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: Number.POSITIVE_INFINITY, refetchOnWindowFocus: false },
      mutations: { retry: false },
    },
  })
  seedQuery?.(client)
  return client
}

export function StoryProviders({
  children,
  initialEntries = ['/'],
  seedQuery,
  withShellLayout = false,
  sidebarCollapsed = false,
}: StoryProvidersProps) {
  const [client] = useState(() => createStoryQueryClient(seedQuery))

  const content = withShellLayout ? (
    <ShellLayoutProvider sidebarCollapsed={sidebarCollapsed}>{children}</ShellLayoutProvider>
  ) : (
    children
  )

  return (
    <QueryClientProvider client={client}>
      <AppMemoryRouter initialEntries={initialEntries}>{content}</AppMemoryRouter>
    </QueryClientProvider>
  )
}

export function withStoryProviders(options: Omit<StoryProvidersProps, 'children'> = {}): Decorator {
  return (Story) => (
    <StoryProviders {...options}>
      <Story />
    </StoryProviders>
  )
}

export function StorySurface({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn('min-h-screen bg-surface-sunken p-6 text-content', className)}>{children}</div>
}

export function ShellStoryFrame({
  children,
  route = '/',
  sidebarCollapsed = false,
  sidebarWidth = layout.sidePanel.defaultWidth,
  contentClassName,
}: ShellStoryFrameProps) {
  return (
    <StoryProviders initialEntries={[route]} withShellLayout sidebarCollapsed={sidebarCollapsed}>
      <div className="app-shell min-h-screen bg-surface-sunken text-content">
        <TitleBar onSearchOpen={() => {}} />
        <ActivityBar onToggleSidebar={() => {}} sidebarCollapsed={sidebarCollapsed} />
        <SidePanel
          collapsed={sidebarCollapsed}
          width={sidebarWidth}
          onResizeStart={() => {}}
          onResizeByKeyboard={() => {}}
          onCollapse={() => {}}
        />
        <main id="main-content" className={cn('overflow-y-auto', layout.mainContent.bg, contentClassName)}>
          {children}
        </main>
        <div
          className={cn(
            'flex items-center justify-between px-2',
            layout.statusBar.height,
            layout.statusBar.bg,
            layout.statusBar.text,
          )}
        >
          <span>Offline</span>
          <span>Storybook review frame</span>
        </div>
      </div>
    </StoryProviders>
  )
}

export function ReviewHeader({ eyebrow, title, description }: { eyebrow: string; title: string; description: string }) {
  return (
    <div className="mb-6 space-y-2">
      <p className="text-content-tertiary text-xs uppercase tracking-[0.18em]">{eyebrow}</p>
      <div className="space-y-1">
        <h1 className="font-bold text-2xl text-content">{title}</h1>
        <p className="max-w-3xl text-content-secondary text-sm">{description}</p>
      </div>
    </div>
  )
}

export function ReviewNote({ children }: { children: ReactNode }) {
  return (
    <Card variant="default" padding="md" className="border-brand-signal/30 bg-brand-signal/5">
      <p className="text-content-secondary text-sm">{children}</p>
    </Card>
  )
}
