import type { TimelineItem } from '../../api/client'
import type { UiScene, UiSceneElement } from '../../api/contracts'

/** Frame item extracted from the timeline */
export type FrameItem = Extract<TimelineItem, { type: 'Frame' }>

/** Scene element with projected viewport coordinates */
export interface ProjectedSceneElement extends UiSceneElement {
  left: number
  top: number
  width: number
  height: number
  title: string | null
}

/** Playback state returned by usePlaybackState */
export interface PlaybackState {
  isPlaying: boolean
  playbackSpeed: number
  currentTime: Date
  startTime: Date
  endTime: Date
  currentFrame: FrameItem | null
  handlePlayPause: () => void
  handleSpeedChange: (speed: number) => void
  handleTimeChange: (time: Date) => void
  handleSkipToStart: () => void
  handleSkipToEnd: () => void
}

/** Props shared between SceneViewport and SceneAssistantPanel */
export interface SceneOverlayProps {
  currentFrame: FrameItem | null
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
