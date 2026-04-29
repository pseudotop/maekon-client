import { Suspense } from 'react'
import { useTranslation } from 'react-i18next'
import { Outlet, useLocation, useNavigate } from 'react-router-dom'
import { Button, Spinner, Tabs } from '../../components/ui'
import { useShellLayoutContext } from '../../contexts/ShellLayoutContext'
import { RouteErrorBoundary, routeTree } from '../../routes'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { SettingsFormProvider, useSettingsFormContext } from './SettingsFormContext'

// Derive settings tabs from routeTree (single source of truth)
const settingsNode = routeTree.find((r) => r.path === '/settings')
const settingsTabs = (settingsNode?.children ?? []).map((c) => ({ id: c.path, label: c.labelKey }))

function SettingsContent() {
  const { t } = useTranslation()
  const { sidebarCollapsed } = useShellLayoutContext()
  const { form, data } = useSettingsFormContext()
  const location = useLocation()
  const navigate = useNavigate()

  if (data.settingsLoading || !form.formData) {
    return (
      <div className="flex h-64 items-center justify-center">
        <Spinner size="lg" className={colors.primary.text} />
        <span className={cn('ml-3', colors.text.secondary)}>{t('common.loading')}</span>
      </div>
    )
  }

  const activeTab = location.pathname.split('/').pop() ?? 'general'
  const translatedTabs = settingsTabs.map((tab) => ({ id: tab.id, label: t(tab.label) }))
  const coreCount = settingsNode?.childGroups?.[0]?.tabs.length ?? 0

  return (
    <div className="min-h-full space-y-6 p-6 pb-28">
      <div className="flex items-center justify-between">
        <h1 className={cn(typography.h1, colors.text.pageTitle)}>
          {t('settings.title')}
          {activeTab !== 'general' && (
            <span className="ml-2 font-normal text-base text-content-tertiary">
              {'>'} {translatedTabs.find((tab) => tab.id === activeTab)?.label}
            </span>
          )}
        </h1>
      </div>

      {sidebarCollapsed && (
        <Tabs
          tabs={translatedTabs}
          activeTab={activeTab}
          onTabChange={(tab) => navigate(`/settings/${tab}`, { replace: true })}
          ariaLabel={t('settings.title')}
          idBase="settings"
          groupBreakAfter={coreCount > 0 ? coreCount - 1 : undefined}
        />
      )}

      {form.hasUnsavedChanges && (
        <div className="pointer-events-none fixed right-6 bottom-10 z-30 flex justify-end">
          <div className="pointer-events-auto flex items-center gap-4 rounded-xl border border-muted bg-surface-overlay px-4 py-3 shadow-2xl">
            <div className="min-w-0">
              <p className={cn(`${typography.weight.semibold} text-sm`, colors.text.primary)}>
                {t('settings.unsavedChanges')}
              </p>
              <p className={cn('text-xs', colors.text.secondary)}>{t('settings.unsavedChangesHint')}</p>
            </div>
            <Button
              type="button"
              variant="secondary"
              size="lg"
              onClick={form.handleRevertChanges}
              disabled={form.saveMutation.isPending}
            >
              {t('settings.revertChanges')}
            </Button>
            <Button
              data-testid="settings-save-floating"
              type="submit"
              form="settings-form"
              variant="primary"
              size="lg"
              isLoading={form.saveMutation.isPending}
              disabled={form.saveDisabled}
            >
              {form.saveMutation.isPending ? t('settings.saving') : t('settings.saveSettings')}
            </Button>
          </div>
        </div>
      )}

      <form id="settings-form" className="space-y-6" onSubmit={form.handleSubmit}>
        <Suspense
          fallback={
            <div className="flex items-center justify-center py-12">
              <Spinner />
            </div>
          }
        >
          <Outlet />
        </Suspense>

        <div className="flex justify-end">
          <Button
            data-testid="settings-save"
            type="submit"
            variant="primary"
            size="lg"
            isLoading={form.saveMutation.isPending}
            disabled={form.saveDisabled}
          >
            {form.saveMutation.isPending ? t('settings.saving') : t('settings.saveSettings')}
          </Button>
        </div>
      </form>
    </div>
  )
}

export default function SettingsLayout() {
  // Provider is OUTSIDE the error boundary so a recovery reset does not
  // remount the provider and destroy unsaved form edits. The boundary still
  // isolates settings-tab crashes per-route — it just can't take out the
  // form state on reset.
  return (
    <SettingsFormProvider>
      <RouteErrorBoundary route="/settings">
        <SettingsContent />
      </RouteErrorBoundary>
    </SettingsFormProvider>
  )
}
