import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { fetchCoachingHistory, fetchGoalProgress, fetchHabitStreaks, updateRegimeGoals } from '../api/coaching'
import { useErrorDisplay } from '../i18n/errorDisplay'
import { addErrorToast, addToast } from './useToast'

export function useCoachingHistory(limit = 50, offset = 0) {
  return useQuery({
    queryKey: ['coaching-history', limit, offset],
    queryFn: () => fetchCoachingHistory(limit, offset),
    staleTime: 30_000,
  })
}

export function useGoalProgress() {
  return useQuery({
    queryKey: ['goal-progress'],
    queryFn: fetchGoalProgress,
    refetchInterval: 30_000,
  })
}

export function useHabitStreaks(days = 7) {
  return useQuery({
    queryKey: ['habit-streaks', days],
    queryFn: () => fetchHabitStreaks(days),
    refetchInterval: 60_000,
  })
}

export function useUpdateGoals() {
  const queryClient = useQueryClient()
  const { t } = useTranslation()
  const describeError = useErrorDisplay()

  return useMutation({
    mutationFn: (goals: Record<string, number>) => updateRegimeGoals(goals),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['goal-progress'] })
      // #8083: regime_goals is also carried in the settings payload (display-only
      // there). Invalidate the settings query so the Settings form's baseline
      // reflects the just-added/removed goal instead of a stale goal list,
      // keeping the two surfaces (Coaching goals section and Settings) consistent.
      queryClient.invalidateQueries({ queryKey: ['settings'] })
      addToast('success', t('coaching.goalsUpdated'))
    },
    onError: (err: unknown) => {
      addErrorToast(describeError(err), undefined, { cause: err })
    },
  })
}
