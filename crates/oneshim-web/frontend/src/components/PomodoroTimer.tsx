/**
 * PomodoroTimer — minimal focus timer with circular progress, start/cancel,
 * auto break-mode transition, and desktop notification on completion.
 */
import { Timer, X } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  cancelPomodoro,
  completePomodoro,
  fetchCurrentPomodoro,
  fetchDesktopPermissionStatus,
  type PomodoroSession,
  type PomodoroStatus,
  requestDesktopNotificationPermission,
  startPomodoro,
} from '../api/client'
import { isStandaloneModeEnabled } from '../api/standalone'
import { addToast } from '../hooks/useToast'
import { colors, dataViz, iconSize, motion, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { IS_MAC, IS_TAURI } from '../utils/platform'
import { Button, Card, CardContent, CardHeader, CardTitle } from './ui'

const DEFAULT_WORK_MINS = 25
const DEFAULT_BREAK_MINS = 5

/** Format seconds as MM:SS */
function formatTime(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) return '00:00'
  const m = Math.floor(secs / 60)
  const s = secs % 60
  return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
}

function statusColor(status: PomodoroStatus): string {
  switch (status) {
    case 'running':
      return dataViz.stroke.good
    case 'on_break':
      return dataViz.stroke.warning
    case 'completed':
      return dataViz.stroke.good
    case 'cancelled':
      return dataViz.stroke.critical
  }
}

/** Circular progress ring for the timer. */
function TimerRing({
  progress,
  status,
  timeText,
  label,
}: {
  progress: number
  status: PomodoroStatus
  timeText: string
  label: string
}) {
  const r = 45
  const circumference = 2 * Math.PI * r
  const offset = circumference * (1 - Math.min(Math.max(progress, 0), 1))
  const color = statusColor(status)

  return (
    <svg width={120} height={120} viewBox="0 0 120 120" aria-hidden="true">
      {/* Background track */}
      <circle cx="60" cy="60" r={r} fill="none" stroke="currentColor" strokeWidth="6" className="text-surface-muted" />
      {/* Progress arc */}
      <circle
        cx="60"
        cy="60"
        r={r}
        fill="none"
        stroke={color}
        strokeWidth="6"
        strokeLinecap="round"
        strokeDasharray={circumference}
        strokeDashoffset={offset}
        transform="rotate(-90 60 60)"
        className={motion.all}
      />
      {/* Time text */}
      <text
        x="60"
        y="55"
        textAnchor="middle"
        dominantBaseline="middle"
        className={`fill-content text-lg ${typography.weight.bold}`}
      >
        {timeText}
      </text>
      {/* Status label */}
      <text x="60" y="75" textAnchor="middle" dominantBaseline="middle" className="fill-content-secondary text-[11px]">
        {label}
      </text>
    </svg>
  )
}

async function desktopNotificationState(): Promise<'granted' | 'blocked' | 'unknown'> {
  if (IS_TAURI && IS_MAC && !isStandaloneModeEnabled()) {
    try {
      const snapshot = await fetchDesktopPermissionStatus()
      return snapshot.notifications.state === 'granted' ? 'granted' : 'blocked'
    } catch {
      return 'unknown'
    }
  }

  if (!('Notification' in window)) {
    return 'unknown'
  }

  if (Notification.permission === 'granted') {
    return 'granted'
  }
  if (Notification.permission === 'denied') {
    return 'blocked'
  }
  return 'unknown'
}

async function requestDesktopNotificationAccess() {
  if (IS_TAURI && IS_MAC && !isStandaloneModeEnabled()) {
    try {
      await requestDesktopNotificationPermission()
    } catch {
      // Native desktop prompt could fail or be unavailable in non-Tauri tests.
    }
    return
  }

  if ('Notification' in window && Notification.permission === 'default') {
    await Notification.requestPermission()
  }
}

async function notifyCompletion(_phase: 'work' | 'break', title: string, body: string) {
  if (!('Notification' in window)) return
  const permissionState = await desktopNotificationState()
  if (permissionState === 'granted') {
    new Notification(title, { body, icon: '/favicon.ico' })
  } else if (permissionState === 'unknown') {
    await requestDesktopNotificationAccess()
  }
}

export default function PomodoroTimer() {
  const { t } = useTranslation()
  const [session, setSession] = useState<PomodoroSession | null>(null)
  const [remaining, setRemaining] = useState(0)
  const [status, setStatus] = useState<PomodoroStatus>('completed')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const prevStatusRef = useRef<PomodoroStatus>('completed')
  const tickRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const describeError = useCallback(
    (value: unknown, fallback: string) => (value instanceof Error && value.message ? value.message : fallback),
    [],
  )
  const statusLabelText = useCallback(
    (value: PomodoroStatus) => {
      switch (value) {
        case 'running':
          return t('focus.pomodoro.focus', 'Focus')
        case 'on_break':
          return t('focus.pomodoro.break', 'Break')
        case 'completed':
          return t('focus.pomodoro.done', 'Done')
        case 'cancelled':
          return t('focus.pomodoro.cancelled', 'Cancelled')
      }
    },
    [t],
  )

  // Derive effective status + remaining from the session snapshot and local clock
  const computeLocal = useCallback((s: PomodoroSession) => {
    const startMs = new Date(s.started_at).getTime()
    const elapsed = Number.isFinite(startMs) ? Math.floor((Date.now() - startMs) / 1000) : 0
    const workSecs = (s.duration_minutes ?? DEFAULT_WORK_MINS) * 60
    const breakSecs = (s.break_minutes ?? DEFAULT_BREAK_MINS) * 60

    if (s.status === 'cancelled') {
      return { status: 'cancelled' as PomodoroStatus, remaining: 0 }
    }
    if (s.status === 'completed') {
      return { status: 'completed' as PomodoroStatus, remaining: 0 }
    }

    if (elapsed >= workSecs + breakSecs) {
      return { status: 'completed' as PomodoroStatus, remaining: 0 }
    }
    if (elapsed >= workSecs) {
      return {
        status: 'on_break' as PomodoroStatus,
        remaining: workSecs + breakSecs - elapsed,
      }
    }
    return {
      status: 'running' as PomodoroStatus,
      remaining: workSecs - elapsed,
    }
  }, [])

  // Tick every second to update the display
  useEffect(() => {
    if (!session) return
    const local = computeLocal(session)
    setStatus(local.status)
    setRemaining(local.remaining)

    tickRef.current = setInterval(() => {
      const loc = computeLocal(session)
      setStatus(loc.status)
      setRemaining(loc.remaining)

      // Detect transitions
      if (prevStatusRef.current === 'running' && loc.status === 'on_break') {
        void notifyCompletion(
          'work',
          t('focus.pomodoro.workCompleteTitle', 'Focus session complete!'),
          t('focus.pomodoro.workCompleteBody', 'Time for a break.'),
        )
      } else if (prevStatusRef.current === 'on_break' && loc.status === 'completed') {
        void notifyCompletion(
          'break',
          t('focus.pomodoro.breakCompleteTitle', 'Break is over!'),
          t('focus.pomodoro.breakCompleteBody', 'Ready to focus again?'),
        )
        // Auto-complete on server
        completePomodoro().catch((e) => {
          console.warn('completePomodoro failed:', e)
          const message = describeError(e, t('focus.pomodoro.syncFailed', 'Failed to sync the completed session.'))
          setError(message)
          addToast('warning', message, 5000)
        })
      }
      prevStatusRef.current = loc.status
    }, 1000)

    return () => {
      if (tickRef.current) clearInterval(tickRef.current)
    }
  }, [session, computeLocal, describeError, t])

  // Load current session on mount
  useEffect(() => {
    fetchCurrentPomodoro()
      .then((s) => {
        if (s) {
          setSession(s)
          prevStatusRef.current = s.status
        }
      })
      .catch((e) => {
        console.warn('fetchCurrentPomodoro failed:', e)
        const message = describeError(e, t('focus.pomodoro.loadFailed', 'Failed to load the current timer.'))
        setError(message)
        addToast('error', message, 5000)
      })
  }, [describeError, t])

  // Request notification permission eagerly
  useEffect(() => {
    void requestDesktopNotificationAccess()
  }, [])

  const handleStart = async () => {
    setError(null)
    setLoading(true)
    try {
      const s = await startPomodoro({
        duration_minutes: DEFAULT_WORK_MINS,
        break_minutes: DEFAULT_BREAK_MINS,
      })
      setSession(s)
      prevStatusRef.current = 'running'
    } catch (e: unknown) {
      const message = describeError(e, t('focus.pomodoro.startFailed', 'Failed to start the timer.'))
      setError(message)
      addToast('error', message, 5000)
    } finally {
      setLoading(false)
    }
  }

  const handleCancel = async () => {
    setError(null)
    setLoading(true)
    try {
      const s = await cancelPomodoro()
      setSession(s)
    } catch (e: unknown) {
      const message = describeError(e, t('focus.pomodoro.cancelFailed', 'Failed to cancel the timer.'))
      setError(message)
      addToast('error', message, 5000)
    } finally {
      setLoading(false)
    }
  }

  const isActive = status === 'running' || status === 'on_break'

  // Compute progress fraction
  const progress = (() => {
    if (!session) return 0
    const totalSecs =
      status === 'running' ? session.duration_minutes * 60 : status === 'on_break' ? session.break_minutes * 60 : 1
    const elapsed = totalSecs - remaining
    return totalSecs > 0 ? elapsed / totalSecs : 1
  })()

  return (
    <Card variant="interactive" padding="sm">
      <CardHeader>
        <CardTitle>
          <Timer className={`mr-2 inline ${iconSize.md}`} />
          {t('focus.pomodoro.title', 'Pomodoro')}
        </CardTitle>
      </CardHeader>
      <CardContent>
        <div className="flex flex-col items-center gap-3">
          <TimerRing
            progress={progress}
            status={status}
            timeText={isActive ? formatTime(remaining) : '--:--'}
            label={isActive ? statusLabelText(status) : t('focus.pomodoro.ready', 'Ready')}
          />
          <span className="sr-only" aria-live="polite">
            {isActive ? `${statusLabelText(status)}: ${formatTime(remaining)}` : t('focus.pomodoro.ready', 'Ready')}
          </span>

          {/* Controls */}
          <div className="flex gap-2">
            {!isActive ? (
              <Button variant="primary" size="sm" onClick={handleStart} disabled={loading}>
                {t('focus.pomodoro.start', { minutes: DEFAULT_WORK_MINS })}
              </Button>
            ) : (
              <Button variant="secondary" size="sm" onClick={handleCancel} disabled={loading}>
                <X className="mr-1 h-3.5 w-3.5" />
                {t('focus.pomodoro.cancel', 'Cancel')}
              </Button>
            )}
          </div>

          {/* Error feedback */}
          {error && <p className={cn('text-xs', colors.semantic.error)}>{error}</p>}

          {/* Session info for completed */}
          {session && status === 'completed' && (
            <p className={cn(typography.small, colors.text.tertiary)}>
              {t('focus.pomodoro.sessionCompleted', 'Session completed')}
            </p>
          )}
        </div>
      </CardContent>
    </Card>
  )
}
