import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Outlet } from 'react-router-dom'
import { fetchSettings, updateSettings } from '../../api/client'
import type { CoachingEvent, GoalProgress } from '../../api/coaching'
import type { AppSettings } from '../../api/contracts'
import HabitTrackerWidget from '../../components/HabitTrackerWidget'
import { Alert, Button, Card, CardContent, CardHeader, CardTitle, Select, Skeleton } from '../../components/ui'
import { useCoachingHistory, useGoalProgress } from '../../hooks/useCoaching'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

export interface CoachingOutletContext {
  history: CoachingEvent[] | undefined
  histLoading: boolean
  goals: GoalProgress[] | undefined
  goalsLoading: boolean
}

const INTERVAL_PRESETS = [
  { value: 60, labelKey: 'coaching.freq1' },
  { value: 120, labelKey: 'coaching.freq2' },
  { value: 300, labelKey: 'coaching.freq5' },
  { value: 600, labelKey: 'coaching.freq10' },
  { value: 900, labelKey: 'coaching.freq15' },
  { value: 1800, labelKey: 'coaching.freq30' },
  { value: 3600, labelKey: 'coaching.freq60' },
]

export default function CoachingLayout() {
  const { t } = useTranslation()
  const { data: history, isLoading: histLoading } = useCoachingHistory()
  const { data: goals, isLoading: goalsLoading } = useGoalProgress()

  const queryClient = useQueryClient()
  const { data: settings, isLoading: settingsLoading } = useQuery({
    queryKey: ['settings'],
    queryFn: fetchSettings,
    staleTime: 30_000,
  })
  const [saved, setSaved] = useState(false)
  // #5707: inline enable mutation for the disabled banner button.
  const [enabling, setEnabling] = useState(false)

  const updateFreq = useMutation({
    mutationFn: updateSettings,
    onSuccess: (data) => {
      queryClient.setQueryData(['settings'], data)
      setSaved(true)
      setTimeout(() => setSaved(false), 1500)
    },
  })

  // #5707: inline handler to enable coaching from the disabled-state banner.
  // Uses the same updateSettings mutation pattern as the frequency selector.
  const handleEnableCoaching = useCallback(async () => {
    setEnabling(true)
    try {
      const fresh = await queryClient.fetchQuery<AppSettings>({ queryKey: ['settings'], queryFn: fetchSettings })
      updateFreq.mutate({ ...fresh, coaching: { ...fresh.coaching, enabled: true } })
    } finally {
      setEnabling(false)
    }
  }, [queryClient, updateFreq])

  const intervalSecs = Object.values(settings?.coaching?.profiles ?? {})[0]?.min_interval_secs ?? 300
  const isCustom = !INTERVAL_PRESETS.some((p) => p.value === intervalSecs)

  const handleIntervalChange = useCallback(
    async (e: React.ChangeEvent<HTMLSelectElement>) => {
      const newInterval = Number(e.target.value)
      const fresh = await queryClient.fetchQuery<AppSettings>({ queryKey: ['settings'], queryFn: fetchSettings })
      const updatedProfiles = Object.fromEntries(
        Object.entries(fresh.coaching.profiles).map(([k, v]) => [k, { ...v, min_interval_secs: newInterval }]),
      )
      updateFreq.mutate({ ...fresh, coaching: { ...fresh.coaching, profiles: updatedProfiles } })
    },
    [queryClient, updateFreq],
  )

  const ctx: CoachingOutletContext = { history, histLoading, goals, goalsLoading }

  // Coaching disabled → show self-healing banner instead of empty history/habit grids.
  const coachingEnabled = settings?.coaching?.enabled ?? true // default true: show content while loading
  const coachingKnownDisabled = !settingsLoading && settings !== undefined && !coachingEnabled

  return (
    <div className="min-h-full p-6">
      <h1 className={cn(typography.h1, colors.text.pageTitle, 'mb-6')}>{t('coaching.title', 'Coaching History')}</h1>

      {/* #5707: Self-healing empty state — shown when coaching.enabled === false.
          Inline Enable button reuses the updateSettings mutation to avoid a full page
          navigation. The banner disappears as soon as settings refetch confirms enabled=true. */}
      {coachingKnownDisabled && (
        <section className="mb-6" data-testid="coaching-disabled-banner">
          <Alert variant="warning" title={t('coaching.disabledBannerTitle', 'Coaching is off')}>
            <p className="mb-3">
              {t('coaching.disabledBannerDesc', 'Enable coaching to receive proactive nudges and track your habits.')}
            </p>
            <Button
              type="button"
              variant="primary"
              size="sm"
              isLoading={enabling || updateFreq.isPending}
              onClick={() => void handleEnableCoaching()}
            >
              {t('coaching.disabledBannerButton', 'Enable coaching')}
            </Button>
          </Alert>
        </section>
      )}

      {/* Coaching frequency control */}
      <section className="mb-6">
        {settingsLoading ? (
          <Skeleton className="h-28 rounded-lg" />
        ) : (
          <Card variant="default" padding="sm">
            <CardHeader>
              <CardTitle>{t('coaching.frequencyTitle')}</CardTitle>
            </CardHeader>
            <CardContent>
              <p className="mb-3 text-content-secondary text-sm">{t('coaching.frequencyDesc')}</p>
              <div className="flex items-center gap-3">
                <Select
                  value={String(intervalSecs)}
                  onChange={handleIntervalChange}
                  disabled={updateFreq.isPending}
                  className="w-48"
                  aria-label={t('coaching.frequencyTitle')}
                >
                  {isCustom && (
                    <option value={intervalSecs}>
                      {t('coaching.frequencyCustom', { min: Math.round(intervalSecs / 60) })}
                    </option>
                  )}
                  {INTERVAL_PRESETS.map((opt) => (
                    <option key={opt.value} value={opt.value}>
                      {t(opt.labelKey)}
                    </option>
                  ))}
                </Select>
                {saved && (
                  <span className="animate-pulse text-semantic-success text-xs">{t('coaching.frequencySaved')}</span>
                )}
              </div>
            </CardContent>
          </Card>
        )}
      </section>

      <Outlet context={ctx} />

      {/* Habit streak tracker */}
      <section className="mb-6">
        <HabitTrackerWidget />
      </section>
    </div>
  )
}
