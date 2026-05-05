import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { CoachingSettings, ProfileConfig, TimeRange } from '../../api/contracts'
import { Button, Card, CardContent, CardHeader, CardTitle, Input } from '../../components/ui'
import { useGoalProgress, useUpdateGoals } from '../../hooks/useCoaching'
import { colors, motion, radius, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { useSettingsFormContext } from '../settings/SettingsFormContext'
import ToggleRow from './ToggleRow'

const COACHING_TONES = ['Direct', 'Gentle', 'DataDriven'] as const

const PROFILE_KEYS = ['FocusGuard', 'TimeAware', 'DeepWorkCoach', 'ContextRestore', 'GoalTracker'] as const

export default function CoachingSettingsTab() {
  const { t } = useTranslation()
  const { form } = useSettingsFormContext()
  const formData = form.formData
  const { data: goals } = useGoalProgress()
  const updateGoalsMutation = useUpdateGoals()
  const [newLabel, setNewLabel] = useState('')
  const [newMinutes, setNewMinutes] = useState(60)
  const [newQuietStart, setNewQuietStart] = useState('22:00')
  const [newQuietEnd, setNewQuietEnd] = useState('08:00')

  const handleCoachingChange = useCallback(
    (field: string, value: unknown) => {
      form.setFormData((prev) => {
        if (!prev) return prev
        return { ...prev, coaching: { ...prev.coaching, [field]: value } }
      })
    },
    [form.setFormData],
  )

  const coaching: CoachingSettings | undefined = formData?.coaching

  // ---- Goals section handlers (existing logic) ----------------------------
  const handleAddGoal = useCallback(() => {
    if (!newLabel.trim()) return
    const current: Record<string, number> = {}
    for (const g of goals ?? []) {
      current[g.regime_label] = g.target_minutes
    }
    current[newLabel.trim()] = newMinutes
    updateGoalsMutation.mutate(current)
    setNewLabel('')
    setNewMinutes(60)
  }, [newLabel, newMinutes, goals, updateGoalsMutation])

  const handleDeleteGoal = useCallback(
    (label: string) => {
      const current: Record<string, number> = {}
      for (const g of goals ?? []) {
        if (g.regime_label !== label) {
          current[g.regime_label] = g.target_minutes
        }
      }
      updateGoalsMutation.mutate(current)
    },
    [goals, updateGoalsMutation],
  )

  // ---- Quiet hours handlers -----------------------------------------------
  const quietHours: TimeRange[] = coaching?.quiet_hours ?? []

  const handleAddQuietHour = useCallback(() => {
    const updated = [...quietHours, { start: newQuietStart, end: newQuietEnd }]
    handleCoachingChange('quiet_hours', updated)
  }, [quietHours, newQuietStart, newQuietEnd, handleCoachingChange])

  const handleDeleteQuietHour = useCallback(
    (index: number) => {
      const updated = quietHours.filter((_, i) => i !== index)
      handleCoachingChange('quiet_hours', updated)
    },
    [quietHours, handleCoachingChange],
  )

  // ---- Tone handler -------------------------------------------------------
  const currentTone = coaching?.tone ?? 'Gentle'

  const handleToneChange = useCallback(
    (tone: string) => {
      handleCoachingChange('tone', tone)
    },
    [handleCoachingChange],
  )

  // ---- Profile toggle handler ---------------------------------------------
  const profiles: Record<string, ProfileConfig> = coaching?.profiles ?? {}

  const handleProfileToggle = useCallback(
    (profileKey: string, enabled: boolean) => {
      const current = profiles[profileKey] ?? { enabled: true, min_interval_secs: 300 }
      const updatedProfiles = { ...profiles, [profileKey]: { ...current, enabled } }
      handleCoachingChange('profiles', updatedProfiles)
    },
    [profiles, handleCoachingChange],
  )

  return (
    <div className="space-y-6">
      {/* Section: Regime Goals (existing) */}
      <Card variant="default" padding="md">
        <CardHeader>
          <CardTitle>{t('coaching.settingsTitle', 'Coaching Goals')}</CardTitle>
        </CardHeader>
        <CardContent>
          {goals && goals.length > 0 ? (
            <div className="mb-4 space-y-2">
              {goals.map((g) => {
                const percent =
                  g.target_minutes > 0
                    ? Math.min(100, Math.round(((g.current_minutes ?? 0) / g.target_minutes) * 100))
                    : 0
                const barColor =
                  percent >= 100 ? 'bg-semantic-success' : percent >= 50 ? 'bg-semantic-warning' : 'bg-semantic-error'

                return (
                  <div key={g.regime_label} className="flex items-center gap-3">
                    <span className={`w-28 truncate text-sm ${typography.weight.medium}`}>{g.regime_label}</span>
                    <div className="flex-1">
                      <div className="h-2 w-full rounded-full bg-surface-elevated">
                        <div
                          className={cn('h-2 rounded-full', motion.all, barColor)}
                          style={{ width: `${percent}%` }}
                        />
                      </div>
                    </div>
                    <span className="w-20 text-right text-content-secondary text-xs">
                      {g.current_minutes ?? 0}/{g.target_minutes} {t('coaching.min', 'min')}
                    </span>
                    <span className={`w-10 text-right text-xs ${typography.weight.semibold}`}>{percent}%</span>
                    <Button variant="ghost" size="sm" onClick={() => handleDeleteGoal(g.regime_label)}>
                      {t('coaching.remove', 'Remove')}
                    </Button>
                  </div>
                )
              })}
            </div>
          ) : (
            <p className="mb-4 text-content-secondary text-sm">
              {t('coaching.noGoals', 'No goals set. Add a regime goal below.')}
            </p>
          )}

          <div className="flex items-end gap-2">
            <div>
              <label htmlFor="coaching-regime-label" className="mb-1 block text-content-secondary text-xs">
                {t('coaching.regimeLabel', 'Regime Label')}
              </label>
              <Input
                id="coaching-regime-label"
                value={newLabel}
                onChange={(e) => setNewLabel(e.target.value)}
                placeholder="e.g. Deep Coding"
                className="w-40"
              />
            </div>
            <div>
              <label htmlFor="coaching-target-minutes" className="mb-1 block text-content-secondary text-xs">
                {t('coaching.targetMinutes', 'Target (min)')}
              </label>
              <Input
                id="coaching-target-minutes"
                type="number"
                value={newMinutes}
                onChange={(e) => setNewMinutes(Number(e.target.value))}
                min={1}
                max={1440}
                className="w-24"
              />
            </div>
            <Button variant="primary" size="sm" onClick={handleAddGoal}>
              {t('coaching.addGoal', 'Add Goal')}
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* Section A: Quiet Hours */}
      <Card variant="default" padding="md">
        <CardHeader>
          <CardTitle>{t('coaching.quietHoursTitle', 'Quiet Hours')}</CardTitle>
        </CardHeader>
        <CardContent>
          <p className={cn('mb-4 text-sm', colors.text.secondary)}>
            {t('coaching.quietHoursDesc', 'Coaching messages are suppressed during these time ranges.')}
          </p>

          {quietHours.length > 0 ? (
            <div className="mb-4 space-y-2">
              {quietHours.map((range, rangeIdx) => (
                <div key={`${range.start}-${range.end}`} className="flex items-center gap-3">
                  <span className={cn('text-sm tabular-nums', colors.text.primary)}>
                    {range.start} &ndash; {range.end}
                  </span>
                  <Button variant="ghost" size="sm" onClick={() => handleDeleteQuietHour(rangeIdx)}>
                    {t('coaching.remove', 'Remove')}
                  </Button>
                </div>
              ))}
            </div>
          ) : (
            <p className="mb-4 text-content-secondary text-sm">
              {t('coaching.noQuietHours', 'No quiet hours configured.')}
            </p>
          )}

          <div className="flex items-end gap-2">
            <div>
              <label htmlFor="quiet-start" className="mb-1 block text-content-secondary text-xs">
                {t('coaching.quietStart', 'Start')}
              </label>
              <input
                id="quiet-start"
                type="time"
                value={newQuietStart}
                onChange={(e) => setNewQuietStart(e.target.value)}
                className={cn('border bg-surface-base px-3 py-2 text-sm', radius.md, colors.text.primary)}
              />
            </div>
            <div>
              <label htmlFor="quiet-end" className="mb-1 block text-content-secondary text-xs">
                {t('coaching.quietEnd', 'End')}
              </label>
              <input
                id="quiet-end"
                type="time"
                value={newQuietEnd}
                onChange={(e) => setNewQuietEnd(e.target.value)}
                className={cn('border bg-surface-base px-3 py-2 text-sm', radius.md, colors.text.primary)}
              />
            </div>
            <Button variant="primary" size="sm" onClick={handleAddQuietHour}>
              {t('coaching.addQuietHour', 'Add')}
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* Section B: Coaching Tone */}
      <Card variant="default" padding="md">
        <CardHeader>
          <CardTitle>{t('coaching.toneTitle', 'Coaching Tone')}</CardTitle>
        </CardHeader>
        <CardContent>
          <fieldset className="space-y-3">
            {COACHING_TONES.map((tone) => (
              <label key={tone} className="flex cursor-pointer items-center gap-3">
                <input
                  type="radio"
                  name="coaching-tone"
                  value={tone}
                  checked={currentTone === tone}
                  onChange={() => handleToneChange(tone)}
                />
                <div>
                  <span className={cn('text-sm', typography.weight.medium, colors.text.primary)}>
                    {t(`coaching.toneOption.${tone}`, tone)}
                  </span>
                  <p className={cn('text-xs', colors.text.tertiary)}>{t(`coaching.toneDesc.${tone}`, '')}</p>
                </div>
              </label>
            ))}
          </fieldset>
        </CardContent>
      </Card>

      {/* Section C: Profile Toggles */}
      <Card variant="default" padding="md">
        <CardHeader>
          <CardTitle>{t('coaching.profilesTitle', 'Coaching Profiles')}</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="space-y-3">
            {PROFILE_KEYS.map((key) => {
              const profile = profiles[key] ?? { enabled: true, min_interval_secs: 300 }
              return (
                <ToggleRow
                  key={key}
                  label={t(`coaching.profiles.${key}`, key)}
                  description={t(`coaching.profileDesc.${key}`, '')}
                  checked={profile.enabled}
                  onChange={(checked) => handleProfileToggle(key, checked)}
                />
              )
            })}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
