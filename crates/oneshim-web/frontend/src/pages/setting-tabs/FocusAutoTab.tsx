import { Plus, X } from 'lucide-react'
import { useCallback, useId, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { AppSettings, FocusAutoSettings, FocusScheduleSettings } from '../../api/contracts'
import { Card, CardTitle, FieldHint, SettingPreview, type SettingPreviewRow } from '../../components/ui'
import { colors, form, iconSize, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { useSettingsFormContext } from '../settings/SettingsFormContext'

const DURATION_PRESETS = [15, 25, 45, 60, 90, 120]
const COOLDOWN_PRESETS = [1, 3, 5, 10, 15]

function normalizeFocusAutoSettings(focusAuto: Partial<FocusAutoSettings> | undefined): FocusAutoSettings {
  return {
    enabled: focusAuto?.enabled ?? false,
    duration_minutes: focusAuto?.duration_minutes ?? 25,
    trigger_apps: focusAuto?.trigger_apps ?? [],
    trigger_schedules: focusAuto?.trigger_schedules ?? [],
    cooldown_secs: focusAuto?.cooldown_secs ?? 300,
  }
}

function isWeekdaySchedule(days: string[]): boolean {
  return days.length === 5 && ['Mon', 'Tue', 'Wed', 'Thu', 'Fri'].every((day) => days.includes(day))
}

export default function FocusAutoTab() {
  const { t } = useTranslation()
  const { form: settingsForm } = useSettingsFormContext()
  const formData = settingsForm.formData ?? ({} as AppSettings)
  const focusAuto = normalizeFocusAutoSettings(formData.focus_auto)
  const durationHintId = useId()
  const cooldownHintId = useId()
  const triggerAppsHintId = useId()
  const schedulesHintId = useId()

  const [newApp, setNewApp] = useState('')

  const updateFocusAuto = useCallback(
    (patch: Partial<FocusAutoSettings>) => {
      settingsForm.handleRootChange('focus_auto' as never, { ...focusAuto, ...patch } as never)
    },
    [settingsForm, focusAuto],
  )

  const addApp = useCallback(() => {
    const trimmed = newApp.trim()
    if (!trimmed || focusAuto.trigger_apps.includes(trimmed)) return
    updateFocusAuto({ trigger_apps: [...focusAuto.trigger_apps, trimmed] })
    setNewApp('')
  }, [newApp, focusAuto, updateFocusAuto])

  const removeApp = useCallback(
    (app: string) => {
      updateFocusAuto({ trigger_apps: focusAuto.trigger_apps.filter((a) => a !== app) })
    },
    [focusAuto, updateFocusAuto],
  )

  const addSchedule = useCallback(() => {
    const entry: FocusScheduleSettings = { start: '09:00', end: '12:00', days: [] }
    updateFocusAuto({ trigger_schedules: [...focusAuto.trigger_schedules, entry] })
  }, [focusAuto, updateFocusAuto])

  const removeSchedule = useCallback(
    (index: number) => {
      updateFocusAuto({ trigger_schedules: focusAuto.trigger_schedules.filter((_, i) => i !== index) })
    },
    [focusAuto, updateFocusAuto],
  )

  const updateSchedule = useCallback(
    (index: number, patch: Partial<FocusScheduleSettings>) => {
      const updated = focusAuto.trigger_schedules.map((s, i) => (i === index ? { ...s, ...patch } : s))
      updateFocusAuto({ trigger_schedules: updated })
    },
    [focusAuto, updateFocusAuto],
  )

  const formatMinutes = (minutes: number) => `${minutes} ${t('focusAuto.minutes')}`

  const scheduleSummary =
    focusAuto.trigger_schedules.length > 0
      ? focusAuto.trigger_schedules
          .map((schedule) => {
            const dayLabel =
              schedule.days.length === 0
                ? t('focusAuto.everyday')
                : isWeekdaySchedule(schedule.days)
                  ? t('focusAuto.weekdays')
                  : t('focusAuto.custom')
            return `${schedule.start}-${schedule.end}, ${dayLabel}`
          })
          .join('; ')
      : t('focusAuto.noSchedules')

  const previewRows: SettingPreviewRow[] = [
    {
      label: t('focusAuto.previewState'),
      value: focusAuto.enabled ? t('focusAuto.active') : t('focusAuto.inactive'),
      tone: focusAuto.enabled ? 'success' : 'muted',
    },
    {
      label: t('focusAuto.duration'),
      value: formatMinutes(focusAuto.duration_minutes),
    },
    {
      label: t('focusAuto.cooldown'),
      value: formatMinutes(focusAuto.cooldown_secs / 60),
    },
    {
      label: t('focusAuto.triggerApps'),
      value: focusAuto.trigger_apps.length > 0 ? focusAuto.trigger_apps.join(', ') : t('focusAuto.noTriggerApps'),
      tone: focusAuto.trigger_apps.length > 0 ? 'default' : 'muted',
    },
    {
      label: t('focusAuto.schedules'),
      value: scheduleSummary,
      tone: focusAuto.trigger_schedules.length > 0 ? 'default' : 'muted',
    },
  ]

  return (
    <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_22rem]">
      <div className="space-y-6">
        {/* Enable toggle */}
        <label className="flex cursor-pointer items-center justify-between">
          <div>
            <span className={cn('text-sm', colors.text.primary)}>{t('focusAuto.enabled')}</span>
            <FieldHint>{t('focusAuto.description')}</FieldHint>
          </div>
          <input
            type="checkbox"
            className={form.checkbox}
            checked={focusAuto.enabled}
            onChange={(e) => updateFocusAuto({ enabled: e.target.checked })}
          />
        </label>

        {/* Duration + Cooldown */}
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label htmlFor="focus-auto-duration" className={form.label}>
              {t('focusAuto.duration')}
            </label>
            <FieldHint id={durationHintId} className="mb-2">
              {t('focusAuto.durationDesc')}
            </FieldHint>
            <select
              id="focus-auto-duration"
              aria-describedby={durationHintId}
              className={cn(
                'w-full rounded-md border border-DEFAULT bg-surface-base px-3 py-2 text-sm',
                colors.text.primary,
              )}
              value={focusAuto.duration_minutes}
              onChange={(e) => updateFocusAuto({ duration_minutes: Number(e.target.value) })}
            >
              {DURATION_PRESETS.map((m) => (
                <option key={m} value={m}>
                  {m} {t('focusAuto.minutes')}
                </option>
              ))}
            </select>
          </div>
          <div>
            <label htmlFor="focus-auto-cooldown" className={form.label}>
              {t('focusAuto.cooldown')}
            </label>
            <FieldHint id={cooldownHintId} className="mb-2">
              {t('focusAuto.cooldownDesc')}
            </FieldHint>
            <select
              id="focus-auto-cooldown"
              aria-describedby={cooldownHintId}
              className={cn(
                'w-full rounded-md border border-DEFAULT bg-surface-base px-3 py-2 text-sm',
                colors.text.primary,
              )}
              value={focusAuto.cooldown_secs / 60}
              onChange={(e) => updateFocusAuto({ cooldown_secs: Number(e.target.value) * 60 })}
            >
              {COOLDOWN_PRESETS.map((m) => (
                <option key={m} value={m}>
                  {m} {t('focusAuto.minutes')}
                </option>
              ))}
            </select>
          </div>
        </div>

        {/* Trigger Apps */}
        <Card variant="default" padding="md">
          <CardTitle className="mb-1">{t('focusAuto.triggerApps')}</CardTitle>
          <FieldHint id={triggerAppsHintId} className="mb-3">
            {t('focusAuto.triggerAppsDesc')}
          </FieldHint>

          <div className="flex flex-wrap gap-2">
            {focusAuto.trigger_apps.map((app) => (
              <span
                key={app}
                className={cn(
                  'inline-flex items-center gap-1 rounded-full px-2.5 py-1 text-xs',
                  'bg-brand/10 text-brand',
                  typography.weight.medium,
                )}
              >
                {app}
                <button
                  type="button"
                  onClick={() => removeApp(app)}
                  className={cn('rounded-full p-0.5 hover:bg-brand/20', motion.colors)}
                  aria-label={t('focusAuto.removeAppLabel', { app })}
                >
                  <X className={iconSize.xs} />
                </button>
              </span>
            ))}
          </div>

          <div className="mt-3 flex gap-2">
            <input
              type="text"
              aria-label={t('focusAuto.appInputLabel')}
              aria-describedby={triggerAppsHintId}
              value={newApp}
              onChange={(e) => setNewApp(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  addApp()
                }
              }}
              placeholder={t('focusAuto.appPlaceholder')}
              className={cn(
                'flex-1 rounded-md border border-DEFAULT bg-surface-base px-3 py-1.5 text-sm',
                colors.text.primary,
                'placeholder:text-content-tertiary',
              )}
            />
            <button
              type="button"
              onClick={addApp}
              disabled={!newApp.trim()}
              className={cn(
                'inline-flex items-center gap-1 rounded-md px-3 py-1.5 text-sm',
                'bg-brand text-content-inverse',
                motion.colors,
                'disabled:cursor-not-allowed disabled:opacity-50',
              )}
            >
              <Plus className={iconSize.xs} />
              {t('focusAuto.addApp')}
            </button>
          </div>
        </Card>

        {/* Schedules */}
        <Card variant="default" padding="md">
          <CardTitle className="mb-1">{t('focusAuto.schedules')}</CardTitle>
          <FieldHint id={schedulesHintId} className="mb-3">
            {t('focusAuto.schedulesDesc')}
          </FieldHint>

          <div className="space-y-2">
            {focusAuto.trigger_schedules.map((sched, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: schedules have no stable ID
              <div key={i} className="flex items-center gap-2">
                <input
                  type="time"
                  value={sched.start}
                  onChange={(e) => updateSchedule(i, { start: e.target.value })}
                  className={cn(
                    'rounded-md border border-DEFAULT bg-surface-base px-2 py-1.5 text-sm',
                    colors.text.primary,
                  )}
                  aria-label={t('focusAuto.scheduleStartLabel', { index: i + 1 })}
                  aria-describedby={schedulesHintId}
                />
                <span className={cn('text-xs', colors.text.secondary)}>–</span>
                <input
                  type="time"
                  value={sched.end}
                  onChange={(e) => updateSchedule(i, { end: e.target.value })}
                  className={cn(
                    'rounded-md border border-DEFAULT bg-surface-base px-2 py-1.5 text-sm',
                    colors.text.primary,
                  )}
                  aria-label={t('focusAuto.scheduleEndLabel', { index: i + 1 })}
                  aria-describedby={schedulesHintId}
                />
                <select
                  value={sched.days.length === 0 ? 'everyday' : isWeekdaySchedule(sched.days) ? 'weekdays' : 'custom'}
                  onChange={(e) => {
                    const v = e.target.value
                    updateSchedule(i, {
                      days: v === 'everyday' ? [] : v === 'weekdays' ? ['Mon', 'Tue', 'Wed', 'Thu', 'Fri'] : sched.days,
                    })
                  }}
                  className={cn(
                    'rounded-md border border-DEFAULT bg-surface-base px-2 py-1.5 text-sm',
                    colors.text.primary,
                  )}
                  aria-label={t('focusAuto.scheduleDaysLabel', { index: i + 1 })}
                  aria-describedby={schedulesHintId}
                >
                  <option value="everyday">{t('focusAuto.everyday')}</option>
                  <option value="weekdays">{t('focusAuto.weekdays')}</option>
                  <option value="custom">{t('focusAuto.custom')}</option>
                </select>
                <button
                  type="button"
                  onClick={() => removeSchedule(i)}
                  className={cn('rounded p-1 hover:bg-surface-muted', motion.colors)}
                  aria-label={t('focusAuto.removeScheduleLabel', { index: i + 1 })}
                >
                  <X className={iconSize.sm} />
                </button>
              </div>
            ))}
          </div>

          <button
            type="button"
            onClick={addSchedule}
            className={cn(
              'mt-3 inline-flex items-center gap-1 text-sm',
              'text-brand hover:text-brand-hover',
              motion.colors,
            )}
          >
            <Plus className={iconSize.xs} />
            {t('focusAuto.addSchedule')}
          </button>
        </Card>
      </div>

      <SettingPreview
        className="xl:sticky xl:top-6 xl:self-start"
        title={t('focusAuto.previewTitle')}
        description={t('focusAuto.previewDescription')}
        rows={previewRows}
        footer={t('focusAuto.previewHint')}
      />
    </div>
  )
}
