import { useQuery } from '@tanstack/react-query'
import { AppWindow, Clock, Image, Monitor, Tag as TagIcon } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { TimelineResponse } from '../../api/client'
import { fetchFrameTags } from '../../api/client'
import TimelineScrubber from '../../components/TimelineScrubber'
import { Badge } from '../../components/ui/Badge'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { iconSize } from '../../styles/tokens'
import type { PlaybackState } from './types'

// ── TimelineScrubberSection ─────────────────────────────────────

interface TimelineScrubberSectionProps {
  timeline: TimelineResponse
  playback: PlaybackState
}

export function TimelineScrubberSection({ timeline, playback }: TimelineScrubberSectionProps) {
  return (
    <div id="section-timeline">
      <TimelineScrubber
        startTime={playback.startTime}
        endTime={playback.endTime}
        currentTime={playback.currentTime}
        isPlaying={playback.isPlaying}
        playbackSpeed={playback.playbackSpeed}
        segments={timeline.segments}
        items={timeline.items}
        onTimeChange={playback.handleTimeChange}
        onPlayPause={playback.handlePlayPause}
        onSpeedChange={playback.handleSpeedChange}
        onSkipToStart={playback.handleSkipToStart}
        onSkipToEnd={playback.handleSkipToEnd}
      />
    </div>
  )
}

// ── FrameCard: frame display with metadata, tags, and injected slots

interface FrameCardProps {
  playback: PlaybackState
  /** Scene viewport (image + element overlays) — rendered above metadata */
  viewportSlot: React.ReactNode
  /** Scene status bar + overlay toggle — rendered below tags */
  statusSlot: React.ReactNode
}

function formatDetailTime(date: Date, locale: string) {
  return date.toLocaleString(locale, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  })
}

export function FrameCard({ playback, viewportSlot, statusSlot }: FrameCardProps) {
  const { t, i18n } = useTranslation()
  const locale = i18n.resolvedLanguage ?? i18n.language
  const { currentFrame } = playback

  const { data: currentFrameTags = [] } = useQuery({
    queryKey: ['frameTags', currentFrame?.id],
    // biome-ignore lint/style/noNonNullAssertion: guarded by enabled: !!currentFrame
    queryFn: () => fetchFrameTags(currentFrame!.id),
    enabled: !!currentFrame,
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {currentFrame
            ? `${currentFrame.app_name} - ${currentFrame.window_title}`
            : t('replay.selectTime', 'Select a time')}
        </CardTitle>
      </CardHeader>
      <CardContent>
        {currentFrame ? (
          <div className="space-y-4">
            {/* Scene viewport (image + element overlays) */}
            {viewportSlot}

            {/* Frame metadata grid */}
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
              <div className="flex items-center space-x-2 text-sm">
                <AppWindow className={`${iconSize.base} text-content-muted`} />
                <span className="text-content-secondary">{currentFrame.app_name}</span>
              </div>
              <div className="flex items-center space-x-2 text-sm">
                <Monitor className={`${iconSize.base} text-content-muted`} />
                <span className="truncate text-content-secondary">{currentFrame.window_title}</span>
              </div>
              <div className="flex items-center space-x-2 text-sm">
                <Clock className={`${iconSize.base} text-content-muted`} />
                <span className="text-content-secondary">
                  {formatDetailTime(new Date(currentFrame.timestamp), locale)}
                </span>
              </div>
              <div className="flex items-center space-x-2 text-sm">
                <span className="text-content-secondary">{t('search.importance', 'Importance')}:</span>
                <Badge
                  color={
                    currentFrame.importance >= 0.7 ? 'success' : currentFrame.importance >= 0.4 ? 'warning' : 'default'
                  }
                >
                  {Math.round(currentFrame.importance * 100)}%
                </Badge>
              </div>
            </div>

            {/* Frame tags */}
            {currentFrameTags.length > 0 && (
              <div className="flex flex-wrap items-center gap-2">
                <TagIcon className={`${iconSize.base} text-content-muted`} />
                {currentFrameTags.map((tag) => (
                  <span
                    key={tag.id}
                    className="rounded-full px-2 py-0.5 text-content-inverse text-xs"
                    style={{ backgroundColor: tag.color }}
                  >
                    {tag.name}
                  </span>
                ))}
              </div>
            )}

            {/* Scene status bar + overlay toggle */}
            {statusSlot}
          </div>
        ) : (
          <div className="flex flex-col items-center justify-center py-12 text-content-secondary">
            <Image className="mb-3 h-12 w-12 opacity-50" />
            <p>{t('replay.noFrames', 'No frames at this time')}</p>
          </div>
        )}
      </CardContent>
    </Card>
  )
}
