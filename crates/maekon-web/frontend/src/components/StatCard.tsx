/**
 *
 */
import type { ReactNode } from 'react'
import { colors, radius, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { Card } from './ui'

export interface StatCardProps {
  title: string
  value: string
  icon: ReactNode
  'data-testid'?: string
}

export default function StatCard({ title, value, icon, 'data-testid': testId }: StatCardProps) {
  return (
    <Card variant="default" padding="md" data-testid={testId}>
      <div className="flex items-center space-x-3">
        <div className={cn('p-2', radius.md, 'bg-brand-signal/10 text-brand-text')}>{icon}</div>
        <div>
          <div className={cn(typography.stat.large, colors.text.primary)}>{value}</div>
          <div className={cn(typography.body, colors.text.secondary)}>{title}</div>
        </div>
      </div>
    </Card>
  )
}
