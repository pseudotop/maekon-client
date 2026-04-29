import { Bell, Camera, CircleAlert, CircleCheckBig, Monitor, RotateCcw } from 'lucide-react'
import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import type { AppSettings, DesktopPermissionState } from '../../api/client'
import { Alert, Badge, Button, Card, CardTitle, Checkbox, GuidancePanel, Input } from '../../components/ui'
import { colors, form, iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { useLoadedFormData, useSettingsFormContext } from '../settings/SettingsFormContext'
import ToggleRow from './ToggleRow'

interface PermissionRowAction {
  label: string
  isLoading?: boolean
  onClick: () => void
}

interface PermissionRow {
  id: string
  icon: ReactNode
  label: string
  description: string
  state: DesktopPermissionState
  action?: PermissionRowAction
}

function badgeColorForState(state: DesktopPermissionState): 'success' | 'warning' | 'default' {
  switch (state) {
    case 'granted':
      return 'success'
    case 'needs_attention':
      return 'warning'
    case 'not_required':
    case 'unavailable':
      return 'default'
  }
}

function describeMacNotificationPermission(
  t: ReturnType<typeof useTranslation>['t'],
  statusReason: string | null | undefined,
) {
  switch (statusReason) {
    case 'macos_notifications_granted':
      return t('settings.permissionMacNotificationsDescGranted', 'Desktop notifications are enabled for Maekon.')
    case 'macos_notifications_not_determined':
      return t(
        'settings.permissionMacNotificationsDescPrompt',
        'Maekon has not requested notification permission yet. Request it so reminders and coaching prompts can appear.',
      )
    case 'macos_notifications_denied':
      return t(
        'settings.permissionMacNotificationsDescDenied',
        'Notifications are turned off for Maekon in macOS System Settings. Enable them manually if you want reminders and coaching prompts.',
      )
    case 'macos_notifications_provisional':
      return t(
        'settings.permissionMacNotificationsDescProvisional',
        'Notifications are available, but macOS may deliver them quietly.',
      )
    case 'macos_notifications_ephemeral':
      return t(
        'settings.permissionMacNotificationsDescEphemeral',
        'Notifications are temporarily available for this session.',
      )
    default:
      return t(
        'settings.permissionMacNotificationsDescFallback',
        'Refresh after changing notification settings in macOS System Settings.',
      )
  }
}

export default function MonitoringTab() {
  const { form: settingsForm, data } = useSettingsFormContext()
  const formData = useLoadedFormData()
  const permissionStatus = data.desktopPermissionStatus ?? null
  const permissionStatusLoading = data.desktopPermissionStatusLoading
  const permissionStatusRefreshing = data.desktopPermissionStatusRefreshing
  const permissionStatusError = data.desktopPermissionStatusError
  const notificationPermissionRequesting = settingsForm.requestNotificationPermissionMutation.isPending
  const onRootChange = (field: string, value: boolean | number) =>
    settingsForm.handleRootChange(field as keyof AppSettings, value)
  const onMonitorChange = settingsForm.handleMonitorChange
  const onRefreshPermissionStatus = data.canQueryDesktopCapabilities
    ? data.handleRefreshDesktopPermissionStatus
    : undefined
  const onRequestNotificationPermission = data.canQueryDesktopCapabilities
    ? () => settingsForm.requestNotificationPermissionMutation.mutate()
    : undefined

  const { t } = useTranslation()
  const permissionStateLabels: Record<DesktopPermissionState, string> = {
    granted: t('settings.permissionStateGranted', 'Ready'),
    needs_attention: t('settings.permissionStateNeedsAttention', 'Attention needed'),
    not_required: t('settings.permissionStateNotRequired', 'Not required'),
    unavailable: t('settings.permissionStateUnavailable', 'Unavailable'),
  }

  // Always render the section when the desktop can be queried, even before the
  // first fetch resolves. This guarantees users always see the permissions UI
  // in the packaged app rather than a silently hidden card if React Query has
  // not transitioned into a loading state yet.
  const showPermissionSection =
    data.canQueryDesktopCapabilities ||
    permissionStatusLoading ||
    permissionStatusRefreshing ||
    Boolean(permissionStatus) ||
    Boolean(permissionStatusError)
  const isMac = permissionStatus?.platform === 'macos'
  const isWindows = permissionStatus?.platform === 'windows'
  const isLinux = permissionStatus?.platform === 'linux'
  const macNotificationReason = permissionStatus?.notifications.status_reason
  const permissionRows: PermissionRow[] = isMac
    ? [
        {
          id: 'accessibility',
          icon: <Monitor className={cn(iconSize.base, 'text-brand-text')} />,
          label: t('onboarding.step2Accessibility'),
          description: t(
            'settings.permissionAccessibilityDesc',
            'Needed for focused element tracking and GUI context analysis.',
          ),
          state: permissionStatus?.accessibility.state ?? 'unavailable',
        },
        {
          id: 'screen-capture',
          icon: <Camera className={cn(iconSize.base, 'text-brand-text')} />,
          label: t('settings.permissionScreenRecordingLabel', 'Screen Recording'),
          description: t(
            'settings.permissionScreenCaptureDesc',
            'Needed to capture screenshots for timeline and insights.',
          ),
          state: permissionStatus?.screen_capture.state ?? 'unavailable',
        },
        {
          id: 'notifications',
          icon: <Bell className={cn(iconSize.base, 'text-brand-text')} />,
          label: t('onboarding.step2Notifications'),
          description: describeMacNotificationPermission(t, macNotificationReason),
          state: permissionStatus?.notifications.state ?? 'unavailable',
          action:
            macNotificationReason === 'macos_notifications_not_determined' && onRequestNotificationPermission
              ? {
                  label: t('settings.permissionMacNotificationsRequestAction', 'Request permission'),
                  isLoading: notificationPermissionRequesting,
                  onClick: onRequestNotificationPermission,
                }
              : undefined,
        },
      ]
    : isWindows
      ? [
          {
            id: 'windows-accessibility',
            icon: <Monitor className={cn(iconSize.base, 'text-brand-text')} />,
            label: t('settings.permissionWindowsAccessibilityLabel', 'UI Automation'),
            description: t(
              'settings.permissionWindowsAccessibilityDesc',
              'Windows accessibility APIs are typically available without a separate approval prompt.',
            ),
            state: permissionStatus?.accessibility.state ?? 'not_required',
          },
          {
            id: 'windows-screen-capture',
            icon: <Camera className={cn(iconSize.base, 'text-brand-text')} />,
            label: t('onboarding.step2ScreenCapture'),
            description: t(
              'settings.permissionWindowsScreenCaptureDesc',
              'Screen capture is usually available when Maekon can access your active desktop session.',
            ),
            state: permissionStatus?.screen_capture.state ?? 'unavailable',
          },
          {
            id: 'windows-notifications',
            icon: <Bell className={cn(iconSize.base, 'text-brand-text')} />,
            label: t('onboarding.step2Notifications'),
            description: t(
              'settings.permissionWindowsNotificationsDesc',
              'Notification delivery depends on your Windows notification settings.',
            ),
            state: permissionStatus?.notifications.state ?? 'not_required',
          },
        ]
      : isLinux
        ? [
            {
              id: 'accessibility-service',
              icon: <Monitor className={cn(iconSize.base, 'text-brand-text')} />,
              label: t('settings.permissionLinuxAccessibilityLabel', 'Accessibility Service'),
              description: t(
                'settings.permissionLinuxAccessibilityDesc',
                'AT-SPI session availability controls focused element context on Linux.',
              ),
              state: permissionStatus?.accessibility.state ?? 'unavailable',
            },
            {
              id: 'linux-screen-capture',
              icon: <Camera className={cn(iconSize.base, 'text-brand-text')} />,
              label: t('settings.permissionLinuxScreenCaptureLabel', 'Desktop Capture'),
              description: t(
                'settings.permissionLinuxScreenCaptureDesc',
                'Screen capture availability depends on your compositor and desktop session settings.',
              ),
              state: permissionStatus?.screen_capture.state ?? 'unavailable',
            },
            {
              id: 'notifications',
              icon: <Bell className={cn(iconSize.base, 'text-brand-text')} />,
              label: t('onboarding.step2Notifications'),
              description: t(
                'settings.permissionLinuxNotificationsDesc',
                'Notification delivery is managed by your desktop session.',
              ),
              state: permissionStatus?.notifications.state ?? 'not_required',
            },
          ]
        : []

  const needsAttention = permissionRows.some((row) => row.state === 'needs_attention')
  const hasUnavailable = permissionRows.some((row) => row.state === 'unavailable')

  return (
    <div className="space-y-6">
      <GuidancePanel
        title={t('settings.guidance.monitoring.title')}
        description={t('settings.guidance.monitoring.description')}
        items={[
          {
            title: t('settings.guidance.monitoring.permissions.title'),
            description: t('settings.guidance.monitoring.permissions.description'),
          },
          {
            title: t('settings.guidance.monitoring.intervals.title'),
            description: t('settings.guidance.monitoring.intervals.description'),
          },
          {
            title: t('settings.guidance.monitoring.privacy.title'),
            description: t('settings.guidance.monitoring.privacy.description'),
          },
        ]}
      />

      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.collectionTitle')}</CardTitle>
        <div className="space-y-4">
          <label className="flex cursor-pointer items-center justify-between">
            <div>
              <span className="text-content-strong">{t('settings.captureEnabled')}</span>
              <p className="text-content-secondary text-xs">{t('settings.captureEnabledDesc')}</p>
            </div>
            <Checkbox
              checked={formData.capture_enabled}
              onChange={(e) => onRootChange('capture_enabled', e.target.checked)}
            />
          </label>

          <div className="grid grid-cols-1 gap-4 pt-4 md:grid-cols-3">
            <div>
              <label htmlFor="settings-idle-threshold" className={form.label}>
                {t('settings.idleThresholdSecs')}
              </label>
              <Input
                id="settings-idle-threshold"
                type="number"
                min={60}
                max={3600}
                step={60}
                value={formData.idle_threshold_secs}
                onChange={(e) => onRootChange('idle_threshold_secs', parseInt(e.target.value, 10) || 300)}
              />
            </div>
            <div>
              <label htmlFor="settings-metrics-interval" className={form.label}>
                {t('settings.metricsIntervalSecs')}
              </label>
              <Input
                id="settings-metrics-interval"
                type="number"
                min={1}
                max={60}
                value={formData.metrics_interval_secs}
                onChange={(e) => onRootChange('metrics_interval_secs', parseInt(e.target.value, 10) || 5)}
              />
            </div>
            <div>
              <label htmlFor="settings-process-interval" className={form.label}>
                {t('settings.processIntervalSecs')}
              </label>
              <Input
                id="settings-process-interval"
                type="number"
                min={5}
                max={300}
                value={formData.process_interval_secs}
                onChange={(e) => onRootChange('process_interval_secs', parseInt(e.target.value, 10) || 10)}
              />
            </div>
          </div>
        </div>
      </Card>

      {showPermissionSection && (
        <Card variant="default" padding="lg">
          <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
            <div className="space-y-1">
              <CardTitle>
                {isMac
                  ? t('settings.permissionSectionTitleMac', 'macOS Permissions')
                  : isLinux
                    ? t('settings.permissionSectionTitleLinux', 'Linux Session Access')
                    : isWindows
                      ? t('settings.permissionSectionTitleWindows', 'Windows Access')
                      : t('settings.permissionSectionTitle', 'Desktop Access')}
              </CardTitle>
              <p className="text-content-secondary text-sm">
                {isMac
                  ? t(
                      'settings.permissionSectionDescMac',
                      'Grant the required macOS permissions so Maekon can capture context reliably.',
                    )
                  : isLinux
                    ? t(
                        'settings.permissionSectionDescLinux',
                        'Linux usually does not require extra prompts, but session services still need to be reachable.',
                      )
                    : isWindows
                      ? t(
                          'settings.permissionSectionDescWindows',
                          'Windows access is usually available without separate prompts, but desktop integrations should still be verified.',
                        )
                      : t(
                          'settings.permissionSectionDesc',
                          'Check whether desktop integrations required by Maekon are currently reachable.',
                        )}
              </p>
            </div>

            {onRefreshPermissionStatus && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                isLoading={permissionStatusRefreshing}
                onClick={onRefreshPermissionStatus}
              >
                {!permissionStatusRefreshing && <RotateCcw className={cn(iconSize.base, 'mr-2')} />}
                {t('settings.permissionRefreshAction', 'Refresh status')}
              </Button>
            )}
          </div>

          {(permissionStatusLoading || permissionStatusRefreshing) && (
            <p className="text-content-secondary text-sm">
              {t('settings.permissionChecking', 'Checking desktop access...')}
            </p>
          )}

          {permissionStatusError && (
            <Alert
              variant="error"
              className="mb-4"
              title={t('settings.permissionCheckFailedTitle', 'Desktop access check failed')}
              icon={<CircleAlert />}
            >
              <p>
                {t(
                  'settings.permissionCheckFailedDesc',
                  'Maekon could not read the current desktop access state. Retry the check after returning from your OS settings.',
                )}
              </p>
              <p className="mt-2 text-content-tertiary text-xs">{permissionStatusError}</p>
            </Alert>
          )}

          {!permissionStatusLoading && !permissionStatusRefreshing && permissionStatus?.platform === 'windows' && (
            <Alert
              variant="info"
              className="mb-4"
              title={t('settings.permissionWindowsTitle', 'Windows access')}
              icon={<CircleCheckBig />}
            >
              {t(
                'settings.permissionWindowsDesc',
                'Windows does not usually require separate OS permissions for accessibility or screen capture. Notifications can still depend on system notification settings.',
              )}
            </Alert>
          )}

          {!permissionStatusLoading && !permissionStatusRefreshing && permissionRows.length > 0 && (
            <>
              <div className="space-y-3">
                {permissionRows.map((row) => (
                  <div
                    key={row.id}
                    className="flex items-start justify-between gap-4 rounded-lg border border-muted bg-surface-inset p-4"
                  >
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="flex h-8 w-8 items-center justify-center rounded-md bg-brand-signal/15">
                          {row.icon}
                        </span>
                        <span className={cn(typography.label, colors.text.primary)}>{row.label}</span>
                      </div>
                      <p className="mt-2 text-content-secondary text-sm">{row.description}</p>
                      {row.action && (
                        <div className="mt-3">
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            isLoading={row.action.isLoading}
                            onClick={row.action.onClick}
                          >
                            {row.action.label}
                          </Button>
                        </div>
                      )}
                    </div>
                    <Badge color={badgeColorForState(row.state)} size="md" className="shrink-0">
                      {permissionStateLabels[row.state]}
                    </Badge>
                  </div>
                ))}
              </div>

              {needsAttention && (
                <Alert
                  variant="warning"
                  className="mt-4"
                  title={t('settings.permissionAttentionTitle', 'Action recommended')}
                  icon={<CircleAlert />}
                >
                  {isMac
                    ? t(
                        'settings.permissionAttentionDescMac',
                        'If a required permission is missing, re-open the setup guide and approve it in System Settings.',
                      )
                    : t(
                        'settings.permissionAttentionDesc',
                        'If a service is unavailable, check your desktop session settings and reopen Maekon.',
                      )}
                </Alert>
              )}

              {!needsAttention && hasUnavailable && (
                <Alert
                  variant="info"
                  className="mt-4"
                  title={t('settings.permissionManualCheckTitle', 'Manual check still required')}
                  icon={<CircleAlert />}
                >
                  {isMac
                    ? t(
                        'settings.permissionManualCheckDescMac',
                        'Some macOS settings, such as notifications, must still be reviewed manually in System Settings.',
                      )
                    : t(
                        'settings.permissionManualCheckDesc',
                        'Some desktop integrations cannot be verified automatically yet. Confirm the related OS settings manually if needed.',
                      )}
                </Alert>
              )}
            </>
          )}
        </Card>
      )}

      <Card id="section-monitoring" variant="default" padding="lg">
        <CardTitle sticky>{t('settings.monitorTitle')}</CardTitle>
        <div className="space-y-4">
          <ToggleRow
            label={t('settings.processMonitoring')}
            description={t('settings.processMonitoringDesc')}
            checked={formData.monitor.process_monitoring}
            onChange={(value) => onMonitorChange('process_monitoring', value)}
          />
          <ToggleRow
            label={t('settings.inputActivity')}
            description={t('settings.inputActivityDesc')}
            checked={formData.monitor.input_activity}
            onChange={(value) => onMonitorChange('input_activity', value)}
          />
          <ToggleRow
            label={t('settings.privacyMode')}
            description={t('settings.privacyModeDesc')}
            checked={formData.monitor.privacy_mode}
            onChange={(value) => onMonitorChange('privacy_mode', value)}
          />
        </div>
      </Card>
    </div>
  )
}
