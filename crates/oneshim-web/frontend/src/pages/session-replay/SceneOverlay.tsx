import { useMutation } from '@tanstack/react-query'
import { Eye, EyeOff } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { executeSceneAction } from '../../api/client'
import type { UiScene } from '../../api/contracts'
import { Alert, Checkbox } from '../../components/ui'
import { Button } from '../../components/ui/Button'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { iconSize, motion, typography } from '../../styles/tokens'
import { resolveImageUrl } from '../../utils/api-base'
import type { FrameItem, ProjectedSceneElement, SceneOverlayProps } from './types'

// ── useSceneState: shared state for viewport + assistant ─────────

export function useSceneState(props: SceneOverlayProps) {
  const { currentFrame, currentScene, overlayAllowed } = props
  const { t } = useTranslation()

  const [showSceneOverlay, setShowSceneOverlay] = useState(true)
  const [selectedSceneElementId, setSelectedSceneElementId] = useState<string | null>(null)
  const [sceneTypeText, setSceneTypeText] = useState('')
  const [allowSensitiveInput, setAllowSensitiveInput] = useState(false)
  const [sceneActionFeedback, setSceneActionFeedback] = useState<{
    success: boolean
    message: string
  } | null>(null)
  const sceneViewportRef = useRef<HTMLDivElement | null>(null)
  const sceneObserverRef = useRef<ResizeObserver | null>(null)
  const [sceneViewportSize, setSceneViewportSize] = useState({ width: 0, height: 0 })

  const sceneCalibrationPassed = props.sceneCalibration?.passed === true
  const sceneCalibrationReasons = Array.isArray(props.sceneCalibration?.reasons) ? props.sceneCalibration.reasons : []

  // Reset scene-specific state on mount (matches original SessionReplay behavior)
  useEffect(() => {
    setSelectedSceneElementId(null)
    setSceneTypeText('')
    setAllowSensitiveInput(false)
    setSceneActionFeedback(null)
  }, [])

  // Disable overlay when not allowed by settings
  useEffect(() => {
    if (!overlayAllowed) {
      setShowSceneOverlay(false)
    }
  }, [overlayAllowed])

  // ResizeObserver for viewport dimensions
  const sceneViewportCallbackRef = useCallback((node: HTMLDivElement | null) => {
    if (sceneObserverRef.current) {
      sceneObserverRef.current.disconnect()
      sceneObserverRef.current = null
    }
    sceneViewportRef.current = node
    if (!node) return

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const { width, height } = entry.contentRect
        if (width > 0 && height > 0) {
          setSceneViewportSize({ width, height })
        }
      }
    })
    observer.observe(node)
    sceneObserverRef.current = observer
  }, [])

  // Project scene elements into viewport coordinates
  const projectedSceneElements = useMemo((): ProjectedSceneElement[] => {
    if (!showSceneOverlay || !currentScene) return []
    const viewportWidth = sceneViewportSize.width
    const viewportHeight = sceneViewportSize.height
    if (viewportWidth <= 0 || viewportHeight <= 0) return []

    const sceneWidth = Math.max(currentScene.screen_width, 1)
    const sceneHeight = Math.max(currentScene.screen_height, 1)
    const scale = Math.min(viewportWidth / sceneWidth, viewportHeight / sceneHeight)
    const renderWidth = sceneWidth * scale
    const renderHeight = sceneHeight * scale
    const offsetX = (viewportWidth - renderWidth) / 2
    const offsetY = (viewportHeight - renderHeight) / 2

    return currentScene.elements
      .map((element) => {
        const left = offsetX + element.bbox_abs.x * scale
        const top = offsetY + element.bbox_abs.y * scale
        const width = Math.max(element.bbox_abs.width * scale, 1)
        const height = Math.max(element.bbox_abs.height * scale, 1)

        return {
          ...element,
          left,
          top,
          width,
          height,
          title: element.role ?? element.label,
        }
      })
      .filter(
        (element) =>
          Number.isFinite(element.left) && Number.isFinite(element.top) && element.width > 1 && element.height > 1,
      )
  }, [showSceneOverlay, currentScene, sceneViewportSize])

  const selectedSceneElement = useMemo(
    () =>
      selectedSceneElementId
        ? (projectedSceneElements.find((element) => element.element_id === selectedSceneElementId) ?? null)
        : null,
    [projectedSceneElements, selectedSceneElementId],
  )

  const selectedActionType = useMemo<'click' | 'type_text'>(() => {
    if (!selectedSceneElement) return 'click'
    const role = selectedSceneElement.role?.toLowerCase() ?? ''
    if (role.includes('input') || role.includes('textbox') || role.includes('field')) {
      return 'type_text'
    }
    return 'click'
  }, [selectedSceneElement])

  const suggestedActionText = useMemo(() => {
    if (!selectedSceneElement) return ''
    const label = selectedSceneElement.label?.trim() || t('replay.unnamedElement', 'Unnamed element')
    const appName = currentFrame?.app_name || t('replay.currentApp', 'current app')
    if (selectedActionType === 'type_text') {
      return t('replay.suggestTypeHint', { label, app: appName, defaultValue: `Type into "${label}" in ${appName}` })
    }
    return t('replay.suggestClickHint', { label, app: appName, defaultValue: `Click "${label}" in ${appName}` })
  }, [selectedSceneElement, currentFrame?.app_name, selectedActionType, t])

  const executeSceneActionMutation = useMutation({
    mutationFn: executeSceneAction,
    onSuccess: (response) => {
      const ok = response.result.success
      setSceneActionFeedback({
        success: ok,
        message: ok
          ? t('replay.actionSuccessWithPolicy', {
              defaultValue: 'Suggested action executed (policy: {{policy}}).',
              policy: response.applied_privacy_policy,
            }) +
            (response.scene_action_override_active ? ` ${t('replay.overrideActiveSuffix', '(override active)')}` : '')
          : response.result.error || t('replay.actionFailed', 'Suggested action failed.'),
      })
    },
    onError: (mutationError) => {
      const message =
        mutationError instanceof Error ? mutationError.message : t('replay.actionFailed', 'Suggested action failed.')
      setSceneActionFeedback({
        success: false,
        message,
      })
    },
  })

  return {
    showSceneOverlay,
    setShowSceneOverlay,
    selectedSceneElementId,
    setSelectedSceneElementId,
    sceneTypeText,
    setSceneTypeText,
    allowSensitiveInput,
    setAllowSensitiveInput,
    sceneActionFeedback,
    setSceneActionFeedback,
    sceneViewportCallbackRef,
    projectedSceneElements,
    selectedSceneElement,
    selectedActionType,
    suggestedActionText,
    executeSceneActionMutation,
    sceneCalibrationPassed,
    sceneCalibrationReasons,
  }
}

export type SceneState = ReturnType<typeof useSceneState>

// ── SceneViewport: frame image + element overlays ────────────────

interface SceneViewportProps {
  currentFrame: FrameItem
  imageLoadFailed: boolean
  onImageLoadFailed: () => void
  scene: SceneState
}

export function SceneViewport({ currentFrame, imageLoadFailed, onImageLoadFailed, scene }: SceneViewportProps) {
  const { t } = useTranslation()
  const {
    showSceneOverlay,
    selectedSceneElementId,
    setSelectedSceneElementId,
    setSceneActionFeedback,
    sceneViewportCallbackRef,
    projectedSceneElements,
  } = scene

  return (
    <div
      ref={sceneViewportCallbackRef}
      className="relative aspect-video overflow-hidden rounded-lg bg-surface-elevated"
    >
      {!imageLoadFailed ? (
        <img
          src={resolveImageUrl(currentFrame.image_url) ?? undefined}
          alt={`Screenshot at ${currentFrame.timestamp}`}
          className="h-full w-full object-contain"
          onError={() => onImageLoadFailed()}
        />
      ) : (
        <div className="flex h-full w-full items-center justify-center px-4 text-center text-content-secondary text-sm">
          {t(
            'replay.imageUnavailable',
            '스크린샷 이미지를 불러오지 못했습니다. file 보존 policy 또는 path state를 확인하세요.',
          )}
        </div>
      )}
      {!imageLoadFailed &&
        showSceneOverlay &&
        projectedSceneElements.map((element) => (
          <button
            type="button"
            key={element.element_id}
            className={`absolute ${motion.colors} ${
              selectedSceneElementId === element.element_id
                ? 'border-2 border-semantic-warning bg-semantic-warning/20'
                : 'border border-brand-signal/90 bg-brand-signal/10 hover:bg-brand-signal/20'
            }`}
            style={{
              left: `${element.left}px`,
              top: `${element.top}px`,
              width: `${element.width}px`,
              height: `${element.height}px`,
            }}
            title={element.title ?? undefined}
            onClick={() => {
              setSelectedSceneElementId(element.element_id)
              setSceneActionFeedback(null)
            }}
          >
            <span className="pointer-events-none absolute -top-5 left-0 max-w-[12rem] truncate rounded bg-brand px-1.5 py-0.5 text-[10px] text-content-inverse shadow">
              {element.title}
            </span>
          </button>
        ))}
    </div>
  )
}

// ── SceneStatusBar: scene info + overlay toggle ─────────────────

interface SceneStatusBarProps {
  currentScene: UiScene | undefined
  sceneFetching: boolean
  sceneError: Error | null
  sceneCalibration: { passed?: boolean; reasons?: string[] } | undefined
  calibrationFetching: boolean
  overlayAllowed: boolean
  imageLoadFailed: boolean
  scene: SceneState
}

export function SceneStatusBar({
  currentScene,
  sceneFetching,
  sceneError,
  sceneCalibration,
  calibrationFetching,
  overlayAllowed,
  imageLoadFailed,
  scene,
}: SceneStatusBarProps) {
  const { t } = useTranslation()
  const { showSceneOverlay, setShowSceneOverlay, sceneCalibrationPassed } = scene

  return (
    <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
      <div className="text-content-secondary text-xs">
        {t('replay.sceneElements', { count: currentScene?.elements.length ?? 0 })}
        {sceneFetching && <span className="ml-2 text-content-muted">{t('common.loading')}</span>}
        {sceneError && <span className="ml-2 text-semantic-warning">{t('replay.sceneUnavailable')}</span>}
        {!sceneError && sceneCalibration && (
          <span className={`ml-2 ${sceneCalibrationPassed ? 'text-semantic-success' : 'text-semantic-warning'}`}>
            {sceneCalibrationPassed
              ? t('replay.calibrationPassed', 'Calibration passed')
              : t('replay.calibrationFailed', 'Calibration failed')}
          </span>
        )}
        {calibrationFetching && (
          <span className="ml-2 text-content-muted">{t('replay.calibrating', 'Calibrating...')}</span>
        )}
      </div>
      <Button
        data-testid="overlay-toggle"
        variant="secondary"
        size="sm"
        onClick={() => setShowSceneOverlay((prev) => !prev)}
        disabled={!currentScene || imageLoadFailed || !overlayAllowed}
      >
        {showSceneOverlay ? (
          <>
            <EyeOff className={`mr-1 ${iconSize.base}`} />
            {t('replay.hideOverlay')}
          </>
        ) : (
          <>
            <Eye className={`mr-1 ${iconSize.base}`} />
            {t('replay.showOverlay')}
          </>
        )}
      </Button>
    </div>
  )
}

// ── SceneAssistantPanel: element selection + action execution ────

interface SceneAssistantPanelProps {
  currentFrame: FrameItem
  currentScene: UiScene | undefined
  sceneIntelligenceEnabled: boolean
  sceneExecutionAllowed: boolean
  sceneCalibration: { passed?: boolean; reasons?: string[] } | undefined
  scene: SceneState
}

export function SceneAssistantPanel({
  currentFrame,
  currentScene,
  sceneIntelligenceEnabled,
  sceneExecutionAllowed,
  sceneCalibration,
  scene,
}: SceneAssistantPanelProps) {
  const { t } = useTranslation()
  const {
    selectedSceneElement,
    selectedActionType,
    suggestedActionText,
    sceneTypeText,
    setSceneTypeText,
    allowSensitiveInput,
    setAllowSensitiveInput,
    sceneActionFeedback,
    setSceneActionFeedback,
    executeSceneActionMutation,
    setSelectedSceneElementId,
    sceneCalibrationPassed,
    sceneCalibrationReasons,
  } = scene

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t('replay.assistantTitle', 'Action Assistant')}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <p className="text-content-secondary text-xs">
          {t('replay.assistantDescription', 'Click a highlighted element to prepare an automation action.')}
        </p>
        {selectedSceneElement ? (
          <>
            <div className="space-y-2 rounded-lg border border-muted p-3">
              <div className={`truncate ${typography.weight.semibold} text-content text-sm`}>
                {selectedSceneElement.label}
              </div>
              <div className="grid grid-cols-2 gap-2 text-content-secondary text-xs">
                <div>
                  {t('replay.role', 'Role')}: {selectedSceneElement.role ?? t('replay.unknown', 'Unknown')}
                </div>
                <div>
                  {t('replay.intent', 'Intent')}: {selectedSceneElement.intent ?? t('replay.unknown', 'Unknown')}
                </div>
                <div className="col-span-2">
                  {t('replay.confidence', 'Confidence')}: {Math.round(selectedSceneElement.confidence * 100)}%
                </div>
              </div>
            </div>

            <div className="break-words rounded-lg bg-surface-elevated px-3 py-2 text-content-strong text-xs">
              <span className={`${typography.weight.medium}`}>{t('replay.suggestedAction', 'Suggested action')}: </span>
              {suggestedActionText}
            </div>

            {!sceneIntelligenceEnabled && (
              <Alert variant="warning" className="py-2 text-xs">
                {t('replay.sceneIntelligenceDisabled', 'Scene intelligence is disabled in settings.')}
              </Alert>
            )}
            {sceneCalibration && !sceneCalibrationPassed && sceneCalibrationReasons.length > 0 && (
              <Alert variant="warning" className="py-2 text-xs">
                {t('replay.calibrationReasons', 'Calibration notes')}: {sceneCalibrationReasons.join('; ')}
              </Alert>
            )}

            {selectedActionType === 'type_text' && (
              <div className="space-y-2">
                <label htmlFor="scene-type-text" className="text-content-secondary text-xs">
                  {t('replay.typeTextLabel', 'Input Text')}
                </label>
                <input
                  id="scene-type-text"
                  value={sceneTypeText}
                  onChange={(e) => setSceneTypeText(e.target.value)}
                  placeholder={t('replay.typeTextPlaceholder', 'Enter text to type')}
                  className="w-full rounded-md border border-DEFAULT bg-surface-overlay px-2 py-1.5 text-content text-sm"
                />
                <label className="flex items-center gap-2 text-content-secondary text-xs">
                  <Checkbox checked={allowSensitiveInput} onChange={(e) => setAllowSensitiveInput(e.target.checked)} />
                  {t('replay.allowSensitiveInput', 'Allow sensitive text input under current privacy policy')}
                </label>
              </div>
            )}

            <div className="flex flex-wrap gap-2">
              <Button
                data-testid="execute-action"
                size="sm"
                isLoading={executeSceneActionMutation.isPending}
                onClick={() => {
                  if (!selectedSceneElement) return
                  executeSceneActionMutation.mutate({
                    command_id: `replay-scene-${currentFrame.id ?? 'frame'}-${Date.now()}`,
                    session_id: `replay-${currentFrame.id ?? 'frame'}`,
                    frame_id: currentFrame.id,
                    scene_id: currentScene?.scene_id,
                    element_id: selectedSceneElement.element_id,
                    action_type: selectedActionType,
                    bbox_abs: selectedSceneElement.bbox_abs,
                    role: selectedSceneElement.role,
                    label: selectedSceneElement.label,
                    text: selectedActionType === 'type_text' ? sceneTypeText : undefined,
                    allow_sensitive_input: selectedActionType === 'type_text' ? allowSensitiveInput : undefined,
                  })
                }}
                disabled={
                  !sceneExecutionAllowed || (selectedActionType === 'type_text' && sceneTypeText.trim().length === 0)
                }
              >
                {t('replay.runSuggestedAction', 'Run Suggested Action')}
              </Button>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => {
                  setSelectedSceneElementId(null)
                  setSceneActionFeedback(null)
                }}
              >
                {t('replay.clearSelection', 'Clear Selection')}
              </Button>
            </div>

            {!sceneExecutionAllowed && (
              <div className="rounded-lg bg-surface-elevated px-3 py-2 text-content-strong text-xs">
                {t('replay.sceneExecutionDisabled', 'Scene action execution is disabled in Settings > Automation.')}
              </div>
            )}

            {sceneActionFeedback && (
              <div
                className={`rounded-lg px-3 py-2 text-xs ${
                  sceneActionFeedback.success
                    ? 'bg-semantic-success/20 text-semantic-success'
                    : 'bg-semantic-error/20 text-semantic-error'
                }`}
              >
                {sceneActionFeedback.message}
              </div>
            )}
          </>
        ) : (
          <div className="rounded-lg border border-DEFAULT border-dashed px-3 py-4 text-content-secondary text-sm">
            {t('replay.noElementSelected', 'No element selected.')}
          </div>
        )}
      </CardContent>
    </Card>
  )
}
