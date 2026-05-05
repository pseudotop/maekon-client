import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { RefreshCw } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Outlet } from 'react-router-dom'
import {
  type AppSettings,
  fetchSettings,
  fetchUpdateStatus,
  postUpdateAction,
  type UpdateAction,
  type UpdateChannel,
  type UpdateStatus,
  updateSettings,
} from '../../api/client'
import { Badge, Button } from '../../components/ui'
import { addToast } from '../../hooks/useToast'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { isSelectableUpdateChannel } from '../../utils/updateChannels'

declare const __APP_VERSION__: string

export interface UpdatesOutletContext {
  settings: AppSettings | undefined
  status: UpdateStatus | undefined
  currentChannel: UpdateChannel
  savingChannel: boolean
  handleChannelChange: (channel: UpdateChannel) => void
  actionMutation: ReturnType<typeof useMutation<Awaited<ReturnType<typeof postUpdateAction>>, Error, UpdateAction>>
  isDownloading: boolean
  versionSummary: {
    current: string
    latest: string
    releaseUrl: string
    releaseName: string | null
    publishedAt: string | null
  } | null
}

export default function UpdatesLayout() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [savingChannel, setSavingChannel] = useState(false)

  const { data: settings } = useQuery<AppSettings>({
    queryKey: ['settings'],
    queryFn: fetchSettings,
    retry: 1,
  })

  const { data: status } = useQuery<UpdateStatus>({
    queryKey: ['update-status'],
    queryFn: fetchUpdateStatus,
    refetchInterval: 15000,
    retry: 1,
  })

  const settingsMutation = useMutation({
    mutationFn: (updated: AppSettings) => updateSettings(updated),
    onSuccess: (data) => {
      queryClient.setQueryData(['settings'], data)
      addToast('success', t('updates.channelSaved', 'Update channel saved'))
    },
    onError: () => {
      addToast('error', t('updates.channelSaveFailed', 'Failed to save update channel'))
    },
    onSettled: () => setSavingChannel(false),
  })

  const actionMutation = useMutation({
    mutationFn: (action: UpdateAction) => postUpdateAction(action),
    onSuccess: (response) => {
      queryClient.setQueryData(['update-status'], response.status)
      queryClient.invalidateQueries({ queryKey: ['update-status'] })
    },
  })

  const handleChannelChange = (channel: UpdateChannel) => {
    if (!settings) return
    if (!isSelectableUpdateChannel(channel)) return
    setSavingChannel(true)
    const updated: AppSettings = {
      ...settings,
      update: { ...settings.update, channel },
    }
    settingsMutation.mutate(updated)
  }

  const currentChannel = settings?.update?.channel ?? 'stable'
  const isDownloading = status?.phase === 'Downloading' || status?.phase === 'Installing'

  const versionSummary = status?.pending
    ? {
        current: status.pending.current_version,
        latest: status.pending.latest_version,
        releaseUrl: status.pending.release_url,
        releaseName: status.pending.release_name,
        publishedAt: status.pending.published_at,
      }
    : null

  const ctx: UpdatesOutletContext = {
    settings,
    status,
    currentChannel,
    savingChannel,
    handleChannelChange,
    actionMutation,
    isDownloading,
    versionSummary,
  }

  return (
    <div className="min-h-full space-y-6 p-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('updates.title')}</h1>
          <Badge color="info">{__APP_VERSION__}</Badge>
        </div>
        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant="secondary"
            size="sm"
            isLoading={actionMutation.isPending}
            onClick={() => actionMutation.mutate('CheckNow')}
          >
            <RefreshCw size={14} className="mr-1.5" />
            {t('updates.checkNow')}
          </Button>
        </div>
      </div>

      <Outlet context={ctx} />
    </div>
  )
}
