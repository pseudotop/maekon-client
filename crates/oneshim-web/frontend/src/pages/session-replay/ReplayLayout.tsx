import { useQuery } from '@tanstack/react-query'
import { AlertCircle } from 'lucide-react'
import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Outlet } from 'react-router-dom'
import { fetchAutomationScene, fetchSceneCalibration, fetchSettings, fetchTimeline } from '../../api/client'
import type { TimelineResponse, UiScene } from '../../api/contracts'
import DateRangePicker from '../../components/DateRangePicker'
import { Alert } from '../../components/ui'
import { Card, CardContent } from '../../components/ui/Card'
import { Spinner } from '../../components/ui/Spinner'
import { colors, iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { usePlaybackState } from './hooks/usePlaybackState'
import { SceneAssistantPanel, type SceneState, useSceneState } from './SceneOverlay'
import { TimelineScrubberSection } from './SessionPlayback'
import type { PlaybackState } from './types'

export interface ReplayOutletContext {
  /**
   * Nullable so the defaultChild (TimelineSection) can own the empty-state
   * UX and this layout can always render <Outlet>, letting RouteRenderer's
   * `/replay` → `/replay/timeline` index redirect fire even when no session
   * has been captured yet. Same empty-state-in-child pattern AuditLayout
   * adopted after the 2026-04-08 routing.spec regression.
   */
  timeline: TimelineResponse | null
  playback: PlaybackState
  scene: SceneState
  currentScene: UiScene | undefined
  sceneFetching: boolean
  sceneError: Error | null
  sceneCalibration: { passed?: boolean; reasons?: string[] } | undefined
  calibrationFetching: boolean
  sceneIntelligenceEnabled: boolean
  overlayAllowed: boolean
  sceneExecutionAllowed: boolean
  imageLoadFailed: boolean
  onImageLoadFailed: () => void
}

export default function ReplayLayout() {
  const { t } = useTranslation()

  const [fromDate, setFromDate] = useState(() => {
    const d = new Date()
    d.setHours(d.getHours() - 1) // default to 1 hour ago
    return d.toISOString().slice(0, 16)
  })
  const [toDate, setToDate] = useState(() => new Date().toISOString().slice(0, 16))

  const {
    data: timelineData,
    isLoading: loading,
    error: timelineError,
  } = useQuery({
    queryKey: ['timeline', fromDate, toDate],
    queryFn: () => fetchTimeline({ from: new Date(fromDate).toISOString(), to: new Date(toDate).toISOString() }),
  })
  const timeline = timelineData ?? null
  const error = timelineError
    ? timelineError instanceof Error
      ? timelineError.message
      : '타임라인 로드 failure'
    : null

  // usePlaybackState's signature accepts `TimelineData | undefined`; pass the
  // raw query data so the hook's own undefined checks handle the loading/
  // empty states without a null-coercion dance.
  const playback = usePlaybackState(timelineData)
  const { currentFrame } = playback

  const [imageLoadFailed, setImageLoadFailed] = useState(false)

  const { data: appSettings } = useQuery({
    queryKey: ['settings'],
    queryFn: fetchSettings,
    staleTime: 30000,
  })

  const {
    data: currentScene,
    isFetching: sceneFetching,
    error: sceneError,
  } = useQuery({
    queryKey: ['automationScene', currentFrame?.id, currentFrame?.app_name],
    queryFn: () =>
      fetchAutomationScene({
        appName: currentFrame?.app_name,
        frameId: currentFrame?.id,
      }),
    enabled: !!currentFrame,
    retry: false,
  })

  const { data: sceneCalibration, isFetching: calibrationFetching } = useQuery({
    queryKey: ['sceneCalibration', currentFrame?.id, currentFrame?.app_name],
    queryFn: () =>
      fetchSceneCalibration({
        appName: currentFrame?.app_name,
        frameId: currentFrame?.id,
      }),
    enabled: !!currentFrame,
    retry: false,
  })

  const sceneIntelligenceEnabled = appSettings?.ai_provider.scene_intelligence.enabled ?? true
  const overlayAllowed = appSettings?.ai_provider.scene_intelligence.overlay_enabled ?? true
  const sceneExecutionAllowed = appSettings?.ai_provider.scene_intelligence.allow_action_execution ?? false

  const scene = useSceneState({
    currentFrame,
    currentScene,
    sceneFetching,
    sceneError,
    sceneCalibration,
    calibrationFetching,
    sceneIntelligenceEnabled,
    overlayAllowed,
    sceneExecutionAllowed,
    imageLoadFailed,
    onImageLoadFailed: () => setImageLoadFailed(true),
  })

  const handleDateRangeChange = useCallback((from: string | undefined, to: string | undefined) => {
    if (from) setFromDate(from)
    if (to) setToDate(to)
  }, [])

  const hasTimelineData = Boolean(timeline && timeline.items.length > 0)

  return (
    <div className="min-h-full space-y-4 p-6">
      {/* Header + date range */}
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('replay.title', 'session 리플레이')}</h1>
        <DateRangePicker
          onRangeChange={handleDateRangeChange}
          initialFrom={fromDate.split('T')[0]}
          initialTo={toDate.split('T')[0]}
        />
      </div>

      {/* Error state */}
      {error && (
        <Alert variant="error" icon={<AlertCircle className={iconSize.md} />}>
          {error}
        </Alert>
      )}

      {/* Loading state */}
      {loading && (
        <div className="flex items-center justify-center py-12">
          <Spinner />
        </div>
      )}

      {/* Main content — Outlet is rendered unconditionally (post-loading) so
          that the `/replay` → `/replay/timeline` index redirect always fires.
          TimelineSection and EventsSection each own the empty state when
          timeline is null or has zero items. The shared chrome
          (TimelineScrubberSection, SceneAssistantPanel sidebar, session
          stats footer) lives here so it persists across both sub-routes —
          the click-then-act flow broke after PR #376 moved
          SceneAssistantPanel into EventsSection but kept SceneViewport in
          TimelineSection. Hoisting the assistant panel back to the layout
          keeps it visible next to the viewport on /replay/timeline while
          still showing it on /replay/events alongside the event log. */}
      {!loading && (
        <>
          {timeline && timeline.items.length > 0 && <TimelineScrubberSection timeline={timeline} playback={playback} />}

          <div className={cn('grid grid-cols-1 gap-4', hasTimelineData && 'lg:grid-cols-3')}>
            <div className={cn('min-w-0 space-y-4', hasTimelineData && 'lg:col-span-2')}>
              <Outlet
                context={
                  {
                    timeline,
                    playback,
                    scene,
                    currentScene,
                    sceneFetching,
                    sceneError,
                    sceneCalibration,
                    calibrationFetching,
                    sceneIntelligenceEnabled,
                    overlayAllowed,
                    sceneExecutionAllowed,
                    imageLoadFailed,
                    onImageLoadFailed: () => setImageLoadFailed(true),
                  } satisfies ReplayOutletContext
                }
              />
            </div>

            {hasTimelineData && currentFrame && (
              <aside className="space-y-4 lg:col-span-1">
                <SceneAssistantPanel
                  currentFrame={currentFrame}
                  currentScene={currentScene}
                  sceneIntelligenceEnabled={sceneIntelligenceEnabled}
                  sceneExecutionAllowed={sceneExecutionAllowed}
                  sceneCalibration={sceneCalibration}
                  scene={scene}
                />
              </aside>
            )}
          </div>

          {timeline && timeline.items.length > 0 && (
            <Card>
              <CardContent className="py-3">
                <div className="grid grid-cols-2 gap-4 text-center sm:grid-cols-5">
                  <div>
                    <p className="text-content-secondary text-xs">{t('replay.duration', 'session 시간')}</p>
                    <p className={`${typography.weight.semibold} text-content text-lg`}>
                      {Math.round(timeline.session.duration_secs / 60)}
                      {t('dashboard.minutes', '분')}
                    </p>
                  </div>
                  <div>
                    <p className="text-content-secondary text-xs">{t('replay.totalEvents', '총 event')}</p>
                    <p className={`${typography.weight.semibold} text-content text-lg`}>
                      {timeline.session.total_events}
                    </p>
                  </div>
                  <div>
                    <p className="text-content-secondary text-xs">{t('replay.totalFrames', '총 frame')}</p>
                    <p className={`${typography.weight.semibold} text-content text-lg`}>
                      {timeline.session.total_frames}
                    </p>
                  </div>
                  <div>
                    <p className="text-content-secondary text-xs">{t('replay.totalIdle', '총 idle')}</p>
                    <p className={`${typography.weight.semibold} text-content text-lg`}>
                      {Math.round(timeline.session.total_idle_secs / 60)}
                      {t('dashboard.minutes', '분')}
                    </p>
                  </div>
                  <div>
                    <p className="text-content-secondary text-xs">{t('replay.apps', '앱 수')}</p>
                    <p className={`${typography.weight.semibold} text-content text-lg`}>
                      {new Set(timeline.segments.map((s) => s.app_name)).size}
                    </p>
                  </div>
                </div>
              </CardContent>
            </Card>
          )}
        </>
      )}
    </div>
  )
}
