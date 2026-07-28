/**
 * #8077-A: frame screenshot with a truthful unavailable-state fallback.
 *
 * Renders the captured screenshot, or — when it cannot be loaded — a
 * placeholder that names the reason (needs re-auth / removed by retention /
 * never captured / temporarily unavailable) and offers the matching recovery
 * action (re-authenticate or retry) instead of a raw broken-image glyph.
 *
 * Loading strategy lives in `useFrameImage` (authenticated blob fetch in the
 * cross-origin Tauri desktop app, same-origin raw `<img>` in browser preview).
 */

import { ImageOff, Lock, RefreshCw } from 'lucide-react'
import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { type FrameImageReason, useFrameImage } from '../hooks/useFrameImage'
import { iconSize, interaction } from '../styles/tokens'
import { cn } from '../utils/cn'
import { Button, Spinner } from './ui'

interface FrameImageProps {
  imageUrl: string | null | undefined
  alt: string
  /** Class applied to the rendered `<img>` (e.g. `h-full w-full object-cover`). */
  imgClassName?: string
  /** Compact placeholder (icon + short label, no buttons) for small thumbnails. */
  compact?: boolean
  /** `loading="lazy"` on the raw `<img>` (grid/list thumbnails). */
  lazy?: boolean
  /**
   * When set, a successfully-rendered image is wrapped in a click target (e.g.
   * "click to enlarge"). The placeholder states are NOT wrapped, so their
   * recovery buttons are never nested inside another button.
   */
  onImageClick?: () => void
  /** Decorative overlay shown over a successfully-rendered clickable image. */
  overlay?: ReactNode
  'data-testid'?: string
}

function reasonCopyKey(reason: FrameImageReason): { key: string; fallback: string } {
  switch (reason) {
    case 'reauth':
      return { key: 'frameImage.reauth', fallback: 'Re-authentication is required to view this screenshot.' }
    case 'deleted':
      return { key: 'frameImage.deleted', fallback: 'This screenshot was removed by the retention policy.' }
    case 'missing':
      return { key: 'frameImage.missing', fallback: 'No screenshot was captured for this frame.' }
    default:
      return { key: 'frameImage.unavailable', fallback: 'Screenshot is temporarily unavailable.' }
  }
}

export default function FrameImage({
  imageUrl,
  alt,
  imgClassName,
  compact = false,
  lazy = false,
  onImageClick,
  overlay,
  'data-testid': testId,
}: FrameImageProps) {
  const { t } = useTranslation()
  const { phase, src, reason, retryable, notifyImgError, retry, reauthenticate, reauthPending } =
    useFrameImage(imageUrl)

  if ((phase === 'raw' || phase === 'ready') && src) {
    const img = (
      <img
        src={src}
        alt={alt}
        className={imgClassName}
        loading={lazy ? 'lazy' : undefined}
        onError={notifyImgError}
        data-testid={testId}
      />
    )
    if (onImageClick) {
      return (
        <button
          type="button"
          onClick={onImageClick}
          className={cn('group relative block h-full w-full cursor-pointer', interaction.focusRing)}
        >
          {img}
          {overlay}
        </button>
      )
    }
    return img
  }

  if (phase === 'loading') {
    return (
      <div
        className={cn(
          'flex h-full w-full items-center justify-center bg-hover text-content-tertiary',
          !compact && 'min-h-[8rem]',
        )}
        data-testid={testId ? `${testId}-loading` : undefined}
      >
        <Spinner size={compact ? 'sm' : 'md'} />
      </div>
    )
  }

  // phase === 'error'
  const effectiveReason: FrameImageReason = reason ?? 'unknown'
  const copy = reasonCopyKey(effectiveReason)
  const Icon = effectiveReason === 'reauth' ? Lock : ImageOff

  if (compact) {
    return (
      <div
        className="flex h-full w-full flex-col items-center justify-center gap-1 bg-hover px-1 text-center text-content-tertiary"
        title={t(copy.key, copy.fallback)}
        data-testid={testId ? `${testId}-unavailable` : undefined}
      >
        <Icon className={iconSize.md} aria-hidden="true" />
        <span className="text-[10px] leading-tight">{t('frameImage.short', 'Unavailable')}</span>
      </div>
    )
  }

  return (
    <div
      className="flex h-full min-h-[8rem] w-full min-w-[12rem] flex-col items-center justify-center gap-3 bg-surface-muted px-4 py-6 text-center"
      data-testid={testId ? `${testId}-unavailable` : undefined}
    >
      <Icon className={cn(iconSize.lg, 'text-content-tertiary')} aria-hidden="true" />
      <p className="max-w-xs text-content-secondary text-sm">{t(copy.key, copy.fallback)}</p>
      {effectiveReason === 'reauth' ? (
        <Button variant="secondary" size="sm" isLoading={reauthPending} onClick={() => void reauthenticate()}>
          <Lock className={cn('mr-1', iconSize.base)} />
          {t('frameImage.reauthAction', 'Re-authenticate')}
        </Button>
      ) : retryable ? (
        <Button variant="secondary" size="sm" onClick={retry}>
          <RefreshCw className={cn('mr-1', iconSize.base)} />
          {t('frameImage.retry', 'Retry')}
        </Button>
      ) : null}
    </div>
  )
}
