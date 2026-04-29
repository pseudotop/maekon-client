/**
 * Replay events sub-route — renders the event log.
 *
 * ReplayLayout owns the scrubber, SceneAssistantPanel sidebar and session
 * statistics footer, so this section is just the alternative main view
 * (event log instead of frame viewport). Empty-state fallback lives here
 * so the layout can always render <Outlet> and its index redirect keeps
 * firing even when timeline data is absent.
 */

import { Play } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import EventLog from '../../components/EventLog'
import { EmptyState } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import type { ReplayOutletContext } from './ReplayLayout'

export default function EventsSection() {
  const { t } = useTranslation()
  const { timeline, playback } = useTypedOutletContext<ReplayOutletContext>('Replay')

  if (!timeline || timeline.items.length === 0) {
    return (
      <EmptyState
        icon={<Play className="h-8 w-8" />}
        title={t('emptyState.replay.title')}
        description={t('emptyState.replay.description')}
      />
    )
  }

  return (
    <div id="section-events" className="min-h-[300px]">
      <EventLog items={timeline.items} currentTime={playback.currentTime} onItemClick={playback.handleTimeChange} />
    </div>
  )
}
