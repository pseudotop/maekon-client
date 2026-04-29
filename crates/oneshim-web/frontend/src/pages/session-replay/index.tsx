import { useQuery } from '@tanstack/react-query'
import { AlertCircle, Play } from 'lucide-react'
import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchAutomationScene, fetchSceneCalibration, fetchSettings, fetchTimeline } from '../../api/client'
import DateRangePicker from '../../components/DateRangePicker'
import EventLog from '../../components/EventLog'
import { Alert, EmptyState } from '../../components/ui'
import { Card, CardContent } from '../../components/ui/Card'
import { Spinner } from '../../components/ui/Spinner'
import { colors, iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { usePlaybackState } from './hooks/usePlaybackState'
import { SceneAssistantPanel, SceneStatusBar, SceneViewport, useSceneState } from './SceneOverlay'
import { FrameCard, TimelineScrubberSection } from './SessionPlayback'

export default function SessionReplay() {
  const { t } = useTranslation()

  const [fromDate, setFromDate] = useState(() => {
    const d = new Date()
    d.setHours(d.getHours() - 1) // default to 1 hour ago
    return d.toISOString().slice(0, 16)
  })
  const [toDate, setToDate] = useState(() => new Date().toISOString().slice(0, 16))

  const {
    data: timeline,
    isLoading: loading,
    error: timelineError,
  } = useQuery({
    queryKey: ['timeline', fromDate, toDate],
    queryFn: () => fetchTimeline({ from: new Date(fromDate).toISOString(), to: new Date(toDate).toISOString() }),
  })
  const error = timelineError
    ? timelineError instanceof Error
      ? timelineError.message
      : '타임라인 로드 failure'
    : null

  const playback = usePlaybackState(timeline)
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

      {/* Empty state */}
      {!loading && (!timeline || timeline.items.length === 0) && !error && (
        <EmptyState
          icon={<Play className="h-8 w-8" />}
          title={t('emptyState.replay.title')}
          description={t('emptyState.replay.description')}
        />
      )}

      {/* Main content */}
      {!loading && timeline && timeline.items.length > 0 && (
        <>
          {/* Timeline scrubber (full width) */}
          <TimelineScrubberSection timeline={timeline} playback={playback} />

          {/* 3-column grid: frame card (cols 1-2) + sidebar (col 3) */}
          <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
            <div className="lg:col-span-2">
              <FrameCard
                playback={playback}
                viewportSlot={
                  currentFrame ? (
                    <SceneViewport
                      currentFrame={currentFrame}
                      imageLoadFailed={imageLoadFailed}
                      onImageLoadFailed={() => setImageLoadFailed(true)}
                      scene={scene}
                    />
                  ) : null
                }
                statusSlot={
                  <SceneStatusBar
                    currentScene={currentScene}
                    sceneFetching={sceneFetching}
                    sceneError={sceneError}
                    sceneCalibration={sceneCalibration}
                    calibrationFetching={calibrationFetching}
                    overlayAllowed={overlayAllowed}
                    imageLoadFailed={imageLoadFailed}
                    scene={scene}
                  />
                }
              />
            </div>

            <div className="space-y-4 lg:col-span-1">
              {currentFrame && (
                <SceneAssistantPanel
                  currentFrame={currentFrame}
                  currentScene={currentScene}
                  sceneIntelligenceEnabled={sceneIntelligenceEnabled}
                  sceneExecutionAllowed={sceneExecutionAllowed}
                  sceneCalibration={sceneCalibration}
                  scene={scene}
                />
              )}

              <div id="section-events" className="min-h-[300px] flex-1">
                <EventLog
                  items={timeline.items}
                  currentTime={playback.currentTime}
                  onItemClick={playback.handleTimeChange}
                />
              </div>
            </div>
          </div>

          {/* Session statistics footer (full width) */}
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
        </>
      )}
    </div>
  )
}
