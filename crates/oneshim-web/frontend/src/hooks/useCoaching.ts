import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { fetchCoachingHistory, fetchGoalProgress, fetchHabitStreaks, updateRegimeGoals } from '../api/coaching'
import { addToast } from './useToast'

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

  return useMutation({
    mutationFn: (goals: Record<string, number>) => updateRegimeGoals(goals),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['goal-progress'] })
      addToast('success', 'Goals updated')
    },
    onError: (err: Error) => {
      addToast('error', err.message)
    },
  })
}
