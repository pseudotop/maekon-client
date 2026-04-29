import { cn } from '../../utils/cn'

export function Skeleton({ className }: { className?: string }) {
  return <div className={cn('animate-pulse rounded bg-surface-muted', className)} aria-hidden="true" />
}

export function StatCardsSkeleton({ count = 4 }: { count?: number }) {
  const placeholderIds = Array.from({ length: count }, (_, index) => `stat-card-skeleton-${index}`)

  return (
    <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
      {placeholderIds.map((placeholderId) => (
        <div key={placeholderId} className="space-y-3 rounded-xl bg-surface-elevated p-4">
          <Skeleton className="h-4 w-20" />
          <Skeleton className="h-8 w-16" />
        </div>
      ))}
    </div>
  )
}

export function ChartSkeleton({ height = 'h-64' }: { height?: string }) {
  return (
    <div className="rounded-xl bg-surface-elevated p-4">
      <Skeleton className="mb-4 h-4 w-32" />
      <Skeleton className={cn('w-full', height)} />
    </div>
  )
}

export function ListSkeleton({ rows = 5 }: { rows?: number }) {
  const placeholderIds = Array.from({ length: rows }, (_, index) => `list-skeleton-${index}`)

  return (
    <div className="space-y-2">
      {placeholderIds.map((placeholderId) => (
        <div key={placeholderId} className="flex items-center gap-3 rounded-lg bg-surface-inset p-3">
          <Skeleton className="h-8 w-8 rounded-full" />
          <div className="flex-1 space-y-2">
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-3 w-1/2" />
          </div>
        </div>
      ))}
    </div>
  )
}
