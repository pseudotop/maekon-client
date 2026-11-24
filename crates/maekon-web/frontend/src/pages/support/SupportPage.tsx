import { Cpu } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import SupportToolsCard from '../../components/SupportToolsCard'
import { Button, Card, CardTitle } from '../../components/ui'
import { colors, iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

// #8079 (CRT-PRV-QC-CJ-00-09 / -05-07): a discoverable top-level Support &
// Diagnostics destination. The diagnostics viewer + bug-report wizard live in
// the shared SupportToolsCard so this page and Settings → General stay in sync.
export default function SupportPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()

  return (
    <div className="min-h-full space-y-6 p-6">
      <div className="space-y-2">
        <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('support.pageTitle')}</h1>
        <p className={cn(typography.body, colors.text.secondary)}>{t('support.pageDescription')}</p>
      </div>

      <SupportToolsCard />

      {/* #13735: System Metrics left the Dashboard's first screen; this is
          where it is found now (also by name in the CommandPalette). */}
      <Card variant="default" padding="lg" data-testid="support-system-metrics">
        <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-start gap-3">
            <Cpu className={cn(iconSize.md, 'mt-0.5 shrink-0 text-brand-text')} aria-hidden="true" />
            <div className="space-y-1">
              <CardTitle>{t('sidebar.systemMetrics')}</CardTitle>
              <p className={cn(typography.body, colors.text.secondary)}>{t('dashboard.systemResources.description')}</p>
            </div>
          </div>
          <Button variant="secondary" size="sm" className="shrink-0" onClick={() => navigate('/monitoring')}>
            {t('dashboard.systemResources.open')}
          </Button>
        </div>
      </Card>
    </div>
  )
}
