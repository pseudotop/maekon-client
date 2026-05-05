/**
 *
 */
import { useTranslation } from 'react-i18next'
import type { ScheduleSettings as ScheduleSettingsType } from '../../api/client'
import { Card, CardTitle, Input } from '../../components/ui'
import { colors, form, motion, typography } from '../../styles/tokens'
import ToggleRow from './ToggleRow'

interface ScheduleSettingsProps {
  schedule: ScheduleSettingsType
  onChange: (field: keyof ScheduleSettingsType, value: boolean | number | string[]) => void
}

export default function ScheduleSettings({ schedule, onChange }: ScheduleSettingsProps) {
  const { t } = useTranslation()

  return (
    <Card variant="default" padding="lg">
      <CardTitle sticky>{t('settings.scheduleTitle')}</CardTitle>
      <div className="space-y-4">
        <ToggleRow
          label={t('settings.scheduleEnabled')}
          description={t('settings.scheduleEnabledDesc')}
          checked={schedule.active_hours_enabled}
          onChange={(v) => onChange('active_hours_enabled', v)}
        />

        <div className={`space-y-4 ${!schedule.active_hours_enabled ? 'pointer-events-none opacity-50' : ''}`}>
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label htmlFor="schedule-start-hour" className={form.label}>
                {t('settings.startHour')}
              </label>
              <Input
                id="schedule-start-hour"
                type="number"
                min={0}
                max={23}
                value={schedule.active_start_hour}
                onChange={(e) => onChange('active_start_hour', parseInt(e.target.value, 10) || 9)}
              />
            </div>
            <div>
              <label htmlFor="schedule-end-hour" className={form.label}>
                {t('settings.endHour')}
              </label>
              <Input
                id="schedule-end-hour"
                type="number"
                min={0}
                max={23}
                value={schedule.active_end_hour}
                onChange={(e) => onChange('active_end_hour', parseInt(e.target.value, 10) || 18)}
              />
            </div>
          </div>

          <div>
            <span className={form.label}>{t('settings.activeDays')}</span>
            <div className="flex flex-wrap gap-2">
              {(['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'] as const).map((day) => {
                const labels: Record<string, string> = {
                  Mon: t('settings.days.Mon'),
                  Tue: t('settings.days.Tue'),
                  Wed: t('settings.days.Wed'),
                  Thu: t('settings.days.Thu'),
                  Fri: t('settings.days.Fri'),
                  Sat: t('settings.days.Sat'),
                  Sun: t('settings.days.Sun'),
                }
                const isActive = schedule.active_days.includes(day)
                return (
                  <button
                    key={day}
                    type="button"
                    onClick={() => {
                      const newDays = isActive
                        ? schedule.active_days.filter((d) => d !== day)
                        : [...schedule.active_days, day]
                      onChange('active_days', newDays)
                    }}
                    className={`rounded-full px-3 py-1 ${typography.weight.medium} text-sm ${motion.colors} ${
                      isActive
                        ? `${colors.primary.DEFAULT} ${colors.text.inverse}`
                        : 'bg-hover text-content-secondary hover:bg-active'
                    }`}
                  >
                    {labels[day]}
                  </button>
                )
              })}
            </div>
          </div>
        </div>

        <div className={`border-t pt-4 ${form.sectionDivider} space-y-4`}>
          <ToggleRow
            label={t('settings.pauseOnLock')}
            description={t('settings.pauseOnLockDesc')}
            checked={schedule.pause_on_screen_lock}
            onChange={(v) => onChange('pause_on_screen_lock', v)}
          />
          <ToggleRow
            label={t('settings.pauseOnBattery')}
            description={t('settings.pauseOnBatteryDesc')}
            checked={schedule.pause_on_battery_saver}
            onChange={(v) => onChange('pause_on_battery_saver', v)}
          />
        </div>
      </div>
    </Card>
  )
}
