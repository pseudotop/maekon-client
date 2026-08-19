import { Download } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import UpdatePanel from '../../components/UpdatePanel'
import { Card, CardTitle, Spinner } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { IS_TAURI } from '../../utils/platform'
import type { UpdatesOutletContext } from './UpdatesLayout'

/** Build metadata returned by the `get_app_build_info` Tauri command. */
interface AppBuildInfo {
  version: string
  build_date: string
  git_sha: string
}

// #8058 P2-3: an always-on About section. Previously the running version was
// only ever shown transiently inside the "Version Information" card (which
// renders only while an update is pending or downloading), and the
// `get_app_build_info` command (version/build_date/git_sha) had no consumer at
// all. This surfaces it persistently. Desktop-only: the build metadata is
// embedded in the Tauri binary, so a standalone/browser view has nothing to show.
// Exported for unit testing (Vitest).
export function AboutCard() {
  const { t } = useTranslation()
  const [info, setInfo] = useState<AppBuildInfo | null>(null)

  useEffect(() => {
    if (!IS_TAURI) return
    let cancelled = false
    import('@tauri-apps/api/core')
      .then(({ invoke }) => invoke<AppBuildInfo>('get_app_build_info'))
      .then((next) => {
        if (!cancelled) setInfo(next)
      })
      // Best-effort: the About panel is informational, so a failed lookup simply
      // renders nothing rather than surfacing an error.
      .catch(() => undefined)
    return () => {
      cancelled = true
    }
  }, [])

  if (!info) return null

  return (
    <Card id="section-about" variant="default" padding="lg">
      <CardTitle className="mb-4">{t('updates.aboutTitle', 'About')}</CardTitle>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <div className="rounded-lg bg-surface-muted p-3">
          <span className="block text-content-secondary text-xs">{t('updates.currentVersion', 'Current')}</span>
          <span className={cn(typography.family.mono, 'text-content-strong text-sm')}>{info.version}</span>
        </div>
        <div className="rounded-lg bg-surface-muted p-3">
          <span className="block text-content-secondary text-xs">{t('updates.aboutBuildDate', 'Build date')}</span>
          <span className={cn(typography.family.mono, 'text-content-strong text-sm')}>{info.build_date}</span>
        </div>
        <div className="rounded-lg bg-surface-muted p-3 sm:col-span-2">
          <span className="block text-content-secondary text-xs">{t('updates.aboutCommit', 'Commit')}</span>
          <span className={cn(typography.family.mono, 'text-content-strong text-sm')}>{info.git_sha}</span>
        </div>
      </div>
    </Card>
  )
}

export default function StatusSection() {
  const { t } = useTranslation()
  const { versionSummary, isDownloading } = useTypedOutletContext<UpdatesOutletContext>('Updates')

  return (
    <>
      {/* Live status panel */}
      <div id="section-status">
        <UpdatePanel />
      </div>

      <AboutCard />

      {/* Version info + Download progress */}
      {(versionSummary || isDownloading) && (
        <Card id="section-version" variant="default" padding="lg">
          <CardTitle className="mb-4">{t('updates.versionInfo', 'Version Information')}</CardTitle>

          {versionSummary && (
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div className="rounded-lg bg-surface-muted p-3">
                <span className="block text-content-secondary text-xs">{t('updates.currentVersion')}</span>
                <span className={cn(typography.family.mono, 'text-content-strong text-sm')}>
                  {versionSummary.current}
                </span>
              </div>
              <div className="rounded-lg bg-surface-muted p-3">
                <span className="block text-content-secondary text-xs">{t('updates.latestVersion')}</span>
                <span className={cn(typography.family.mono, 'text-content-strong text-sm')}>
                  {versionSummary.latest}
                </span>
              </div>
              {versionSummary.releaseName && (
                <div className="rounded-lg bg-surface-muted p-3 sm:col-span-2">
                  <span className="block text-content-secondary text-xs">{t('updates.releaseName', 'Release')}</span>
                  <span className="text-content-strong text-sm">{versionSummary.releaseName}</span>
                  {versionSummary.publishedAt && (
                    <span className="ml-2 text-content-secondary text-xs">
                      {new Date(versionSummary.publishedAt).toLocaleDateString()}
                    </span>
                  )}
                </div>
              )}
              {versionSummary.releaseUrl && (
                <div className="sm:col-span-2">
                  <a
                    href={versionSummary.releaseUrl}
                    target="_blank"
                    rel="noreferrer"
                    className="text-brand-text text-sm underline"
                  >
                    {t('updates.openRelease')}
                  </a>
                </div>
              )}
            </div>
          )}

          {isDownloading && (
            <div className="mt-4 flex items-center gap-3 rounded-lg border border-brand-muted bg-brand-muted/10 p-3">
              <Download size={18} className="shrink-0 text-brand-text" />
              <div className="flex-1">
                <span className="block text-content-strong text-sm">
                  {t('updates.downloadInProgress', 'Downloading update...')}
                </span>
                <div className="mt-1.5 h-1.5 w-full overflow-hidden rounded-full bg-surface-muted">
                  <div className="h-full animate-pulse rounded-full bg-brand-text" style={{ width: '60%' }} />
                </div>
              </div>
              <Spinner size="sm" />
            </div>
          )}
        </Card>
      )}
    </>
  )
}
