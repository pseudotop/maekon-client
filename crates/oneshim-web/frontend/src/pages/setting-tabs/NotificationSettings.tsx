/**
 *
 */
import { useTranslation } from 'react-i18next'
import type { NotificationSettings as NotificationSettingsType } from '../../api/client'
import { Card, CardTitle, Checkbox, GuidancePanel, Input } from '../../components/ui'
import { colors, form, typography } from '../../styles/tokens'

interface NotificationSettingsProps {
  notification: NotificationSettingsType
  onChange: (field: keyof NotificationSettingsType, value: number | boolean) => void
}

export default function NotificationSettings({ notification, onChange }: NotificationSettingsProps) {
  const { t } = useTranslation()

  return (
    <div className="space-y-6">
      <GuidancePanel
        title={t('settings.guidance.notifications.title')}
        description={t('settings.guidance.notifications.description')}
        items={[
          {
            title: t('settings.guidance.notifications.permission.title'),
            description: t('settings.guidance.notifications.permission.description'),
          },
          {
            title: t('settings.guidance.notifications.thresholds.title'),
            description: t('settings.guidance.notifications.thresholds.description'),
          },
          {
            title: t('settings.guidance.notifications.usage.title'),
            description: t('settings.guidance.notifications.usage.description'),
          },
        ]}
      />

      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.notifTitle')}</CardTitle>

        {/* UI note */}
        <label className={`mb-6 flex cursor-pointer items-center justify-between border-b pb-4 ${form.sectionDivider}`}>
          <div>
            <span className={`${colors.text.secondary} ${typography.weight.medium}`}>{t('settings.notifEnabled')}</span>
            <p className={colors.text.tertiary}>{t('settings.notifEnabledDesc')}</p>
          </div>
          <Checkbox checked={notification.enabled} onChange={(e) => onChange('enabled', e.target.checked)} />
        </label>

        <div className={`space-y-6 ${!notification.enabled ? 'pointer-events-none opacity-50' : ''}`}>
          {/* idle notification */}
          <div className="grid grid-cols-1 items-start gap-4 md:grid-cols-2">
            <label className="flex cursor-pointer items-center">
              <Checkbox
                checked={notification.idle_notification}
                onChange={(e) => onChange('idle_notification', e.target.checked)}
                className="mr-3"
              />
              <div>
                <span className={colors.text.secondary}>{t('settings.notifIdle')}</span>
                <p className={colors.text.tertiary}>{t('settings.notifIdleDesc')}</p>
              </div>
            </label>
            <div>
              <label htmlFor="notif-idle-threshold" className={form.label}>
                {t('settings.notifIdleThreshold')}
              </label>
              <Input
                id="notif-idle-threshold"
                type="number"
                min={5}
                max={120}
                value={notification.idle_notification_mins}
                onChange={(e) => onChange('idle_notification_mins', parseInt(e.target.value, 10) || 30)}
                disabled={!notification.idle_notification}
              />
            </div>
          </div>

          {/* UI note */}
          <div className={`grid grid-cols-1 items-start gap-4 border-t pt-4 md:grid-cols-2 ${form.sectionDivider}`}>
            <label className="flex cursor-pointer items-center">
              <Checkbox
                checked={notification.long_session_notification}
                onChange={(e) => onChange('long_session_notification', e.target.checked)}
                className="mr-3"
              />
              <div>
                <span className={colors.text.secondary}>{t('settings.notifLongSession')}</span>
                <p className={colors.text.tertiary}>{t('settings.notifLongSessionDesc')}</p>
              </div>
            </label>
            <div>
              <label htmlFor="notif-long-session-threshold" className={form.label}>
                {t('settings.notifLongSessionThreshold')}
              </label>
              <Input
                id="notif-long-session-threshold"
                type="number"
                min={30}
                max={240}
                value={notification.long_session_mins}
                onChange={(e) => onChange('long_session_mins', parseInt(e.target.value, 10) || 60)}
                disabled={!notification.long_session_notification}
              />
            </div>
          </div>

          {/* UI note */}
          <div className={`grid grid-cols-1 items-start gap-4 border-t pt-4 md:grid-cols-2 ${form.sectionDivider}`}>
            <label className="flex cursor-pointer items-center">
              <Checkbox
                checked={notification.high_usage_notification}
                onChange={(e) => onChange('high_usage_notification', e.target.checked)}
                className="mr-3"
              />
              <div>
                <span className={colors.text.secondary}>{t('settings.notifHighUsage')}</span>
                <p className={colors.text.tertiary}>{t('settings.notifHighUsageDesc')}</p>
              </div>
            </label>
            <div>
              <label htmlFor="notif-high-usage-threshold" className={form.label}>
                {t('settings.notifHighUsageThreshold')}
              </label>
              <Input
                id="notif-high-usage-threshold"
                type="number"
                min={50}
                max={99}
                value={notification.high_usage_threshold}
                onChange={(e) => onChange('high_usage_threshold', parseInt(e.target.value, 10) || 90)}
                disabled={!notification.high_usage_notification}
              />
            </div>
          </div>
        </div>
      </Card>
    </div>
  )
}
