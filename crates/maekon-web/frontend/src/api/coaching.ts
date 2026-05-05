import { resolveApiUrl } from '../utils/api-base'
import { IS_TAURI } from '../utils/platform'

export interface CoachingEvent {
  event_id: string
  trigger_type: string
  profile_name: string
  regime_id: string | null
  message_template: string
  personalized_message: string | null
  shown_at: string
  dismissed_at: string | null
  dismiss_action: string | null
  feedback_type: string | null
  feedback_score: number | null
}

export interface GoalProgress {
  regime_label: string
  current_minutes: number
  target_minutes: number
  percentage: number
  display_color: string
}

export interface HabitStreak {
  regime_label: string
  date: string
  minutes_logged: number
  target_minutes: number
  met: boolean
}

async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

export async function fetchCoachingHistory(limit = 50, offset = 0): Promise<CoachingEvent[]> {
  if (IS_TAURI) {
    return tauriInvoke<CoachingEvent[]>('get_coaching_history', { limit, offset })
  }
  const url = await resolveApiUrl(`/api/coaching/history?limit=${limit}&offset=${offset}`)
  const response = await fetch(url)
  if (!response.ok) throw new Error(`Failed to fetch coaching history: ${response.statusText}`)
  return response.json()
}

export async function fetchGoalProgress(): Promise<GoalProgress[]> {
  if (IS_TAURI) {
    return tauriInvoke<GoalProgress[]>('get_goal_progress')
  }
  const url = await resolveApiUrl('/api/coaching/goals')
  const response = await fetch(url)
  if (!response.ok) throw new Error(`Failed to fetch goal progress: ${response.statusText}`)
  return response.json()
}

export async function fetchHabitStreaks(days = 7): Promise<HabitStreak[]> {
  if (IS_TAURI) {
    return tauriInvoke<HabitStreak[]>('get_habit_streaks', { days })
  }
  const url = await resolveApiUrl(`/api/coaching/habits?days=${days}`)
  const response = await fetch(url)
  if (!response.ok) throw new Error(`Failed to fetch habit streaks: ${response.statusText}`)
  return response.json()
}

export async function updateRegimeGoals(goals: Record<string, number>): Promise<void> {
  if (IS_TAURI) {
    return tauriInvoke<void>('update_regime_goals', { goals })
  }
  const url = await resolveApiUrl('/api/coaching/goals')
  const response = await fetch(url, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ goals }),
  })
  if (!response.ok) throw new Error(`Failed to update goals: ${response.statusText}`)
}
