import { useTranslation } from 'react-i18next'
import { DurableTaskPanel } from '../../components/DurableTaskPanel'
import { typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

/**
 * Tasks & recovery route (#8892). Mounts the previously-orphaned
 * {@link DurableTaskPanel} so confirmed to-dos and reviewable candidates have a
 * user-reachable main-window surface, complementing the tracking-panel recovery
 * card that generates them.
 */
export default function TasksPage() {
  const { t } = useTranslation()
  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-6">
      <header className="flex flex-col gap-1">
        <h1 className={cn(typography.h2, 'text-content')}>{t('tasks.heading', 'Tasks & recovery')}</h1>
        <p className="text-content-secondary text-sm">
          {t('tasks.subtitle', 'Review suggested next steps and manage your confirmed to-dos.')}
        </p>
      </header>
      <DurableTaskPanel />
    </div>
  )
}
