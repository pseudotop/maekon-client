/**
 * TrackingScheduleSettings — A.20 real implementation.
 *
 * Standalone page (not embedded in SettingsFormContext) that manages the
 * tracking schedule via its own React Query fetch + mutation loop.
 *
 * Endpoints used:
 *   GET  /api/tracking-schedule         → TrackingScheduleConfig
 *   PUT  /api/tracking-schedule         → TrackingScheduleConfig
 *   GET  /api/tracking-schedule/status  → TrackingScheduleStatus
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  getTrackingSchedule,
  getTrackingScheduleStatus,
  setTrackingSchedule,
  type TrackingScheduleConfig,
  type TrackingScheduleStatus,
  type TrackingWindow,
} from '../../api/client'
import type { Weekday } from '../../api/contracts'
import { Button, Card, CardTitle, GuidancePanel, Input, Select } from '../../components/ui'
import { colors, form, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const HH_MM_RE = /^([01]\d|2[0-3]):([0-5]\d)$/

/** All 7 days in backend-canonical order, matching Rust's Weekday enum. */
const ALL_DAYS: Weekday[] = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

/** A small curated list of IANA time zones shown in the dropdown. */
const IANA_ZONES = [
  'Local',
  'America/New_York',
  'America/Chicago',
  'America/Denver',
  'America/Los_Angeles',
  'America/Sao_Paulo',
  'Europe/London',
  'Europe/Paris',
  'Europe/Berlin',
  'Europe/Moscow',
  'Asia/Dubai',
  'Asia/Kolkata',
  'Asia/Seoul',
  'Asia/Tokyo',
  'Asia/Shanghai',
  'Asia/Singapore',
  'Australia/Sydney',
  'Pacific/Auckland',
]

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Internal representation — pairs a server TrackingWindow with a stable local ID. */
interface WindowEntry {
  id: number
  win: TrackingWindow
}

let windowIdCounter = 0
function nextWindowId(): number {
  return ++windowIdCounter
}

function defaultWindowEntry(): WindowEntry {
  return { id: nextWindowId(), win: { start: '09:00', end: '17:00', days_of_week: [...ALL_DAYS], label: '' } }
}

function toEntries(windows: TrackingWindow[]): WindowEntry[] {
  return windows.map((win) => ({ id: nextWindowId(), win }))
}

function fromEntries(entries: WindowEntry[]): TrackingWindow[] {
  return entries.map((e) => e.win)
}

/** Extract HH:MM from an ISO-8601 timestamp string or plain HH:MM. */
function parseEndsAt(endsAt: string | null): string {
  if (!endsAt) return ''
  // ISO-8601 e.g. "2026-04-24T17:00:00+09:00"
  const m = endsAt.match(/T(\d{2}:\d{2})/)
  if (m) return m[1]
  // Already HH:MM
  if (HH_MM_RE.test(endsAt)) return endsAt
  return endsAt
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function TrackingScheduleSettings() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()

  // ── remote state ──────────────────────────────────────────────────────────
  const { data: config } = useQuery<TrackingScheduleConfig>({
    queryKey: ['tracking-schedule'],
    queryFn: getTrackingSchedule,
    retry: 1,
  })

  const { data: status } = useQuery<TrackingScheduleStatus>({
    queryKey: ['tracking-schedule-status'],
    queryFn: getTrackingScheduleStatus,
    retry: 1,
  })

  // ── local form state (mirrors server config once loaded) ──────────────────
  const [enabled, setEnabled] = useState<boolean>(false)
  const [entries, setEntries] = useState<WindowEntry[]>([])
  const [timezone, setTimezone] = useState<string>('Local')
  // errors keyed by stable window ID (not array index)
  const [errors, setErrors] = useState<Record<number, { start?: string; end?: string }>>({})
  const [initialised, setInitialised] = useState(false)

  // Seed local state once config arrives (only on first load).
  // Wrapped in useEffect to avoid render-phase setState (React 18 Strict Mode tearing).
  useEffect(() => {
    if (config && !initialised) {
      setEnabled(config.enabled)
      setEntries(toEntries(config.windows))
      setTimezone(config.timezone ?? 'Local')
      setInitialised(true)
    }
  }, [config, initialised])

  // ── mutation ───────────────────────────────────────────────────────────────
  const saveMutation = useMutation({
    mutationFn: (cfg: TrackingScheduleConfig) => setTrackingSchedule(cfg),
    onSuccess: (data) => {
      queryClient.setQueryData(['tracking-schedule'], data)
    },
  })

  // ── handlers ───────────────────────────────────────────────────────────────
  function addWindow() {
    setEntries((prev) => [...prev, defaultWindowEntry()])
  }

  function removeWindow(id: number) {
    setEntries((prev) => prev.filter((e) => e.id !== id))
    setErrors((prev) => {
      const next = { ...prev }
      delete next[id]
      return next
    })
  }

  function updateWindow(id: number, field: keyof TrackingWindow, value: string) {
    setEntries((prev) => prev.map((e) => (e.id === id ? { ...e, win: { ...e.win, [field]: value } } : e)))
    // Validate on every change for time fields: show error immediately if invalid
    if (field === 'start' || field === 'end') {
      if (!HH_MM_RE.test(value)) {
        setErrors((prev) => ({
          ...prev,
          [id]: { ...prev[id], [field]: t('trackingSchedule.validationHhmm') },
        }))
      } else {
        setErrors((prev) => {
          const next = { ...prev }
          if (next[id]) {
            delete next[id][field]
            if (!next[id].start && !next[id].end) delete next[id]
          }
          return next
        })
      }
    }
  }

  /** Toggle a single day in a window's days_of_week list. */
  function toggleDay(id: number, day: Weekday) {
    setEntries((prev) =>
      prev.map((e) => {
        if (e.id !== id) return e
        const current = e.win.days_of_week
        const next = current.includes(day) ? current.filter((d) => d !== day) : [...current, day]
        return { ...e, win: { ...e.win, days_of_week: next } }
      }),
    )
  }

  /** Validate a single HH:MM field and record the error keyed by stable ID. Returns true if valid. */
  function validateTimeField(id: number, field: 'start' | 'end', value: string): boolean {
    if (!HH_MM_RE.test(value)) {
      setErrors((prev) => ({
        ...prev,
        [id]: { ...prev[id], [field]: t('trackingSchedule.validationHhmm') },
      }))
      return false
    }
    return true
  }

  function handleSave() {
    // Validate all time fields before submitting
    let valid = true
    for (const entry of entries) {
      if (!validateTimeField(entry.id, 'start', entry.win.start)) valid = false
      if (!validateTimeField(entry.id, 'end', entry.win.end)) valid = false
    }
    if (!valid) return
    saveMutation.mutate({ enabled, windows: fromEntries(entries), timezone })
  }

  // ── status pill ─────────────────────────────────────────────────────────
  const activeNow = status?.active_now === true
  const endsAt = parseEndsAt(status?.ends_at ?? null)

  // ── render ─────────────────────────────────────────────────────────────
  return (
    <div className="space-y-6">
      <GuidancePanel
        title={t('settings.guidance.trackingSchedule.title')}
        description={t('settings.guidance.trackingSchedule.description')}
        items={[
          {
            title: t('settings.guidance.trackingSchedule.windows.title'),
            description: t('settings.guidance.trackingSchedule.windows.description'),
          },
          {
            title: t('settings.guidance.trackingSchedule.timezone.title'),
            description: t('settings.guidance.trackingSchedule.timezone.description'),
          },
          {
            title: t('settings.guidance.trackingSchedule.status.title'),
            description: t('settings.guidance.trackingSchedule.status.description'),
          },
        ]}
      />

      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('trackingSchedule.title')}</CardTitle>

        {/* Active-now pill */}
        {activeNow && (
          <div
            className={cn(
              'mb-4 inline-flex items-center gap-2 rounded-full px-3 py-1 text-sm',
              'bg-success/10 text-success',
            )}
            aria-live="polite"
          >
            <span className="h-2 w-2 rounded-full bg-success" />
            {t('trackingSchedule.activeNow', { time: endsAt })}
          </div>
        )}

        <div className="space-y-6">
          {/* Enabled toggle */}
          <div className="flex items-center justify-between">
            <span className={cn(typography.weight.medium, colors.text.secondary)}>{t('trackingSchedule.enabled')}</span>
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => setEnabled(e.target.checked)}
              className={form.checkbox}
            />
          </div>

          {/* Timezone dropdown */}
          <div>
            <label htmlFor="tracking-timezone" className={form.label}>
              {t('trackingSchedule.timezoneLabel')}
            </label>
            <Select
              id="tracking-timezone"
              value={timezone}
              onChange={(e) => setTimezone(e.target.value)}
              aria-label={t('trackingSchedule.timezoneLabel')}
            >
              {IANA_ZONES.map((tz) => (
                <option key={tz} value={tz}>
                  {tz === 'Local' ? t('trackingSchedule.timezoneLocal') : tz}
                </option>
              ))}
            </Select>
          </div>

          {/* Window list */}
          <div className="space-y-4">
            {entries.length === 0 ? (
              <p className={colors.text.secondary}>{t('trackingSchedule.noWindows')}</p>
            ) : (
              entries.map(({ id, win }) => (
                <div key={id} className="rounded-lg border border-muted bg-surface-secondary p-4">
                  <div className="grid grid-cols-2 gap-4">
                    {/* Start time */}
                    <div>
                      <label htmlFor={`win-start-${id}`} className={form.label}>
                        {t('trackingSchedule.startTime')}
                      </label>
                      <Input
                        id={`win-start-${id}`}
                        type="text"
                        aria-label={t('trackingSchedule.startTime')}
                        value={win.start}
                        placeholder="HH:MM"
                        onChange={(e) => updateWindow(id, 'start', e.target.value)}
                        onBlur={(e) => validateTimeField(id, 'start', e.target.value)}
                      />
                      {errors[id]?.start && <p className="mt-1 text-danger text-xs">{errors[id].start}</p>}
                    </div>

                    {/* End time */}
                    <div>
                      <label htmlFor={`win-end-${id}`} className={form.label}>
                        {t('trackingSchedule.endTime')}
                      </label>
                      <Input
                        id={`win-end-${id}`}
                        type="text"
                        aria-label={t('trackingSchedule.endTime')}
                        value={win.end}
                        placeholder="HH:MM"
                        onChange={(e) => updateWindow(id, 'end', e.target.value)}
                        onBlur={(e) => validateTimeField(id, 'end', e.target.value)}
                      />
                      {errors[id]?.end && <p className="mt-1 text-danger text-xs">{errors[id].end}</p>}
                    </div>
                  </div>

                  {/* Day-of-week checkboxes */}
                  <div className="mt-3">
                    <span className={form.label}>{t('trackingSchedule.daysLabel')}</span>
                    <div className="mt-1 flex flex-wrap gap-3">
                      {ALL_DAYS.map((day) => (
                        <label key={day} className="flex cursor-pointer select-none items-center gap-1 text-sm">
                          <input
                            type="checkbox"
                            className={form.checkbox}
                            aria-label={t(`trackingSchedule.${day.toLowerCase()}`)}
                            checked={win.days_of_week.includes(day)}
                            onChange={() => toggleDay(id, day)}
                          />
                          {t(`trackingSchedule.${day.toLowerCase()}`)}
                        </label>
                      ))}
                    </div>
                  </div>

                  {/* Label */}
                  <div className="mt-3">
                    <label htmlFor={`win-label-${id}`} className={form.label}>
                      {t('trackingSchedule.label')}
                    </label>
                    <Input
                      id={`win-label-${id}`}
                      type="text"
                      value={win.label ?? ''}
                      onChange={(e) => updateWindow(id, 'label', e.target.value)}
                    />
                  </div>

                  {/* Remove button */}
                  <div className="mt-3 flex justify-end">
                    <Button type="button" variant="secondary" size="sm" onClick={() => removeWindow(id)}>
                      {t('trackingSchedule.removeWindow')}
                    </Button>
                  </div>
                </div>
              ))
            )}
          </div>

          {/* Add window */}
          <Button type="button" variant="secondary" size="md" onClick={addWindow}>
            {t('trackingSchedule.addWindow')}
          </Button>

          {/* Save */}
          <div className="flex justify-end border-muted border-t pt-4">
            <Button type="button" variant="primary" size="md" isLoading={saveMutation.isPending} onClick={handleSave}>
              {t('trackingSchedule.save')}
            </Button>
          </div>

          {/* Save error feedback */}
          {saveMutation.isError && (
            <p className="text-danger text-sm" role="alert">
              {t('trackingSchedule.saveError')}
            </p>
          )}
        </div>
      </Card>
    </div>
  )
}

export default TrackingScheduleSettings
