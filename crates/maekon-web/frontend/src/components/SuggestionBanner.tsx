import { Brain, Check, Coffee, Focus, MessageSquare, RotateCcw, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchUnifiedSuggestions, type SuggestionDto, submitUnifiedSuggestionFeedback } from '../api/client'
import { addToast } from '../hooks/useToast'
import { iconSize, motion, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { Button } from './ui/Button'

const SUGGESTION_ICONS: Record<string, { icon: typeof Focus; color: string; bgColor: string; borderColor: string }> = {
  NeedFocusTime: {
    icon: Focus,
    color: 'text-semantic-info',
    bgColor: 'bg-semantic-info/20',
    borderColor: 'border-semantic-info',
  },
  TakeBreak: {
    icon: Coffee,
    color: 'text-semantic-warning',
    bgColor: 'bg-semantic-warning/20',
    borderColor: 'border-semantic-warning',
  },
  RestoreContext: {
    icon: RotateCcw,
    color: 'text-semantic-info',
    bgColor: 'bg-semantic-info/20',
    borderColor: 'border-semantic-info',
  },
  PatternDetected: {
    icon: Brain,
    color: 'text-semantic-success',
    bgColor: 'bg-semantic-success/20',
    borderColor: 'border-semantic-success',
  },
  ExcessiveCommunication: {
    icon: MessageSquare,
    color: 'text-semantic-warning',
    bgColor: 'bg-semantic-warning/20',
    borderColor: 'border-semantic-warning',
  },
  WORK_GUIDANCE: {
    icon: Focus,
    color: 'text-semantic-info',
    bgColor: 'bg-semantic-info/20',
    borderColor: 'border-semantic-info',
  },
  EMAIL_DRAFT: {
    icon: MessageSquare,
    color: 'text-semantic-info',
    bgColor: 'bg-semantic-info/20',
    borderColor: 'border-semantic-info',
  },
  PRODUCTIVITY_TIP: {
    icon: Coffee,
    color: 'text-semantic-warning',
    bgColor: 'bg-semantic-warning/20',
    borderColor: 'border-semantic-warning',
  },
  WORKFLOW_OPTIMIZATION: {
    icon: RotateCcw,
    color: 'text-semantic-success',
    bgColor: 'bg-semantic-success/20',
    borderColor: 'border-semantic-success',
  },
  CONTEXT_BASED: {
    icon: Brain,
    color: 'text-semantic-success',
    bgColor: 'bg-semantic-success/20',
    borderColor: 'border-semantic-success',
  },
  // #5696: the wire serializes SuggestionType as SCREAMING_SNAKE_CASE — the
  // CamelCase keys above are legacy/unreachable for API rows. These three are
  // what the rule producer emits after type differentiation.
  TAKE_BREAK: {
    icon: Coffee,
    color: 'text-semantic-warning',
    bgColor: 'bg-semantic-warning/20',
    borderColor: 'border-semantic-warning',
  },
  NEED_FOCUS_TIME: {
    icon: Focus,
    color: 'text-semantic-info',
    bgColor: 'bg-semantic-info/20',
    borderColor: 'border-semantic-info',
  },
  RESTORE_CONTEXT: {
    icon: RotateCcw,
    color: 'text-semantic-info',
    bgColor: 'bg-semantic-info/20',
    borderColor: 'border-semantic-info',
  },
}

function getSuggestionMessage(suggestion: SuggestionDto, t: (key: string) => string): string {
  if (suggestion.content.trim()) {
    return suggestion.content
  }

  switch (suggestion.suggestion_type) {
    case 'NeedFocusTime':
      return t('focus.suggestions.needFocusTime').replace('{minutes}', '25')
    case 'TakeBreak':
      return t('focus.suggestions.takeBreak').replace('{minutes}', '90')
    case 'RestoreContext':
      return t('focus.suggestions.restoreContext').replace('{app}', 'app')
    case 'PatternDetected':
      return t('focus.suggestions.patternDetected').replace('{description}', '')
    case 'ExcessiveCommunication':
      return t('focus.suggestions.excessiveCommunication')
    default:
      return t('focus.suggestions.default')
  }
}

export default function SuggestionBanner() {
  const { t } = useTranslation()
  const [suggestions, setSuggestions] = useState<SuggestionDto[]>([])
  const [currentIndex, setCurrentIndex] = useState(0)
  const [loading, setLoading] = useState(true)
  const [dismissed, setDismissed] = useState<Set<string>>(new Set())

  useEffect(() => {
    fetchUnifiedSuggestions()
      .then((data) => {
        const pending = data.filter((s) => !s.acted_at && !s.dismissed_at)
        setSuggestions(pending)
      })
      .catch((e) => {
        console.warn('fetchUnifiedSuggestions failed:', e)
        addToast('warning', t('focus.suggestions.loadFailed', 'Failed to load focus suggestions.'), 5000)
      })
      .finally(() => setLoading(false))
  }, [t])

  const pendingSuggestions = suggestions.filter((s) => !dismissed.has(s.suggestion_id))

  const currentSuggestion = pendingSuggestions[currentIndex]

  // biome-ignore lint/correctness/useExhaustiveDependencies: intentionally keyed on id and shown_at only — full object would cause unnecessary re-fires
  useEffect(() => {
    if (currentSuggestion && !currentSuggestion.shown_at) {
      submitUnifiedSuggestionFeedback(currentSuggestion.suggestion_id, 'shown').catch((e) =>
        console.warn('submitUnifiedSuggestionFeedback(shown) failed:', e),
      )
    }
  }, [currentSuggestion?.suggestion_id, currentSuggestion?.shown_at])

  if (loading || pendingSuggestions.length === 0) {
    return null
  }

  const suggestionConfig = SUGGESTION_ICONS[currentSuggestion.suggestion_type] || SUGGESTION_ICONS.PatternDetected
  const Icon = suggestionConfig.icon

  const handleDismiss = async () => {
    try {
      await submitUnifiedSuggestionFeedback(currentSuggestion.suggestion_id, 'dismissed')
      setDismissed(new Set(dismissed).add(currentSuggestion.suggestion_id))
      if (currentIndex >= pendingSuggestions.length - 1) {
        setCurrentIndex(Math.max(0, currentIndex - 1))
      }
    } catch (e) {
      console.warn('submitUnifiedSuggestionFeedback(dismissed) failed:', e)
      addToast('error', t('focus.suggestions.dismissFailed', 'Failed to dismiss the suggestion.'), 5000)
    }
  }

  const handleMarkDone = async () => {
    try {
      await submitUnifiedSuggestionFeedback(currentSuggestion.suggestion_id, 'acted')
      setDismissed(new Set(dismissed).add(currentSuggestion.suggestion_id))
      if (currentIndex >= pendingSuggestions.length - 1) {
        setCurrentIndex(Math.max(0, currentIndex - 1))
      }
    } catch (e) {
      console.warn('submitUnifiedSuggestionFeedback(acted) failed:', e)
      addToast('error', t('focus.suggestions.actFailed', 'Failed to mark the suggestion as done.'), 5000)
    }
  }

  const handleNext = () => {
    setCurrentIndex((prev) => (prev + 1) % pendingSuggestions.length)
  }

  return (
    <div
      className={cn(
        suggestionConfig.bgColor,
        'border-l-4',
        suggestionConfig.borderColor,
        'mb-4 flex items-center gap-4 rounded-r-lg px-4 py-3',
      )}
    >
      {/* UI note */}
      <div className={cn('flex-shrink-0', suggestionConfig.color)}>
        <Icon className={iconSize.lg} />
      </div>

      {/* UI note */}
      <div className="min-w-0 flex-1">
        <p className={cn('truncate text-content', typography.label)}>{getSuggestionMessage(currentSuggestion, t)}</p>
        {pendingSuggestions.length > 1 && (
          <p className={cn('mt-0.5 text-content-secondary', typography.caption)}>
            {currentIndex + 1} / {pendingSuggestions.length}
          </p>
        )}
      </div>

      {/* UI note */}
      <div className="flex flex-shrink-0 items-center gap-2">
        {/* UI note */}
        <Button variant="primary" size="sm" onClick={handleMarkDone} className="flex items-center gap-1">
          <Check className={iconSize.xs} />
          {t('focus.suggestions.act')}
        </Button>

        {/* UI note */}
        {pendingSuggestions.length > 1 && (
          <Button variant="ghost" size="sm" onClick={handleNext}>
            {t('common.next')}
          </Button>
        )}

        {/* UI note */}
        <button
          type="button"
          onClick={handleDismiss}
          className={cn('rounded-md p-1 text-content-secondary hover:bg-hover', motion.colors)}
          title={t('common.close')}
        >
          <X className={iconSize.base} />
        </button>
      </div>
    </div>
  )
}
