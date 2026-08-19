import { useTranslation } from 'react-i18next'
import SupportToolsCard from '../../components/SupportToolsCard'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

// #8079 (CRT-PRV-QC-CJ-00-09 / -05-07): a discoverable top-level Support &
// Diagnostics destination. The diagnostics viewer + bug-report wizard live in
// the shared SupportToolsCard so this page and Settings → General stay in sync.
export default function SupportPage() {
  const { t } = useTranslation()

  return (
    <div className="min-h-full space-y-6 p-6">
      <div className="space-y-2">
        <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('support.pageTitle')}</h1>
        <p className={cn(typography.body, colors.text.secondary)}>{t('support.pageDescription')}</p>
      </div>

      <SupportToolsCard />
    </div>
  )
}
