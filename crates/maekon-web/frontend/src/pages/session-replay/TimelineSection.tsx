/**
 * Replay timeline sub-route — renders the frame card (viewport + metadata).
 *
 * ReplayLayout owns the scrubber, the SceneAssistantPanel sidebar and the
 * session statistics footer so that the click-then-act flow survives the
 * `/replay` → `/replay/timeline|events` split. This section is intentionally
 * narrow: just the main frame viewport. Empty-state fallback lives here so
 * ReplayLayout can always render <Outlet> and its index redirect keeps
 * firing.
 */

import { Play } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { EmptyState } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import type { ReplayOutletContext } from './ReplayLayout'
import { SceneStatusBar, SceneViewport } from './SceneOverlay'
import { FrameCard } from './SessionPlayback'

export default function TimelineSection() {
  const { t } = useTranslation()
  const {
    timeline,
    playback,
    scene,
    currentScene,
    sceneFetching,
    sceneError,
    sceneCalibration,
    calibrationFetching,
    overlayAllowed,
    imageLoadFailed,
  } = useTypedOutletContext<ReplayOutletContext>('Replay')

  if (!timeline || timeline.items.length === 0) {
    return (
      <EmptyState
        icon={<Play className="h-8 w-8" />}
        title={t('emptyState.replay.title')}
        description={t('emptyState.replay.description')}
      />
    )
  }

  const { currentFrame } = playback

  return (
    <FrameCard
      playback={playback}
      viewportSlot={
        currentFrame ? (
          <SceneViewport
            currentFrame={currentFrame}
            imageLoadFailed={imageLoadFailed}
            onImageLoadFailed={() => {
              /* handled by layout */
            }}
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
  )
}
