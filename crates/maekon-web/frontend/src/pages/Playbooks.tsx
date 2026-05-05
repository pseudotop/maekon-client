import { useQuery } from '@tanstack/react-query'
import {
  BookOpen,
  ChevronDown,
  ChevronUp,
  Filter,
  Layers,
  MessageSquare,
  PlayCircle,
  Settings2,
  ShieldCheck,
} from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import {
  type CoachingTemplateDto,
  fetchCoachingTemplates,
  fetchPresetLibrary,
  type PresetSummaryDto,
} from '../api/client'
import { EmptyState, GuidanceEmptyState, ListSkeleton, Select } from '../components/ui'
import { Badge } from '../components/ui/Badge'
import { Card, CardContent, CardHeader, CardTitle } from '../components/ui/Card'
import { colors, iconSize, motion, typography } from '../styles/tokens'
import type { BadgeColor } from '../styles/variants'
import { cn } from '../utils/cn'

// ── Types ────────────────────────────────────────────────────

type PlaybookTab = 'coaching' | 'presets'
type TemplatePart = { kind: 'text'; value: string } | { kind: 'variable'; value: string }

const TEMPLATE_VARIABLE_PATTERN = /\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g

function splitTemplateText(text: string): TemplatePart[] {
  const parts: TemplatePart[] = []
  let lastIndex = 0

  for (const match of text.matchAll(TEMPLATE_VARIABLE_PATTERN)) {
    const matchIndex = match.index ?? 0
    if (matchIndex > lastIndex) {
      parts.push({ kind: 'text', value: text.slice(lastIndex, matchIndex) })
    }
    parts.push({ kind: 'variable', value: match[1] })
    lastIndex = matchIndex + match[0].length
  }

  if (lastIndex < text.length) {
    parts.push({ kind: 'text', value: text.slice(lastIndex) })
  }

  return parts.length > 0 ? parts : [{ kind: 'text', value: text }]
}

function TemplateText({ text }: { text: string }) {
  const { t } = useTranslation()
  const parts = useMemo(() => splitTemplateText(text), [text])

  return (
    <>
      {parts.map((part, index) =>
        part.kind === 'variable' ? (
          <span
            // biome-ignore lint/suspicious/noArrayIndexKey: parsed text segments have stable order for a single immutable template
            key={index}
            className={cn(
              'mx-0.5 inline-flex rounded border border-muted bg-surface-muted px-1.5 py-0.5 align-baseline text-[0.78em] text-content-strong',
              typography.family.mono,
            )}
          >
            <span className="sr-only">{t('playbooks.variableLabel', { name: part.value })}</span>
            <code aria-hidden="true">{part.value}</code>
          </span>
        ) : (
          // biome-ignore lint/suspicious/noArrayIndexKey: parsed text segments have stable order for a single immutable template
          <span key={index}>{part.value}</span>
        ),
      )}
    </>
  )
}

// ── Category badge color map ─────────────────────────────────

const categoryBadgeColor: Record<string, BadgeColor> = {
  Productivity: 'success',
  Workflow: 'info',
  AppManagement: 'warning',
  Custom: 'purple',
}

// ── Tone badge color map ─────────────────────────────────────

const toneBadgeColor: Record<string, BadgeColor> = {
  motivating: 'success',
  gentle: 'info',
  curious: 'purple',
  direct: 'warning',
}

// ── Coaching Template Card ───────────────────────────────────

interface CoachingCardProps {
  template: CoachingTemplateDto
}

function CoachingCard({ template }: CoachingCardProps) {
  const { t } = useTranslation()
  const [expanded, setExpanded] = useState(false)
  const toneColor: BadgeColor = toneBadgeColor[template.tone] ?? 'default'
  const isLong = template.text.length > 120

  return (
    <Card variant="default" padding="sm">
      <CardHeader>
        <div className="flex flex-wrap items-center gap-2">
          <Badge color="primary" size="sm">
            {template.profile}
          </Badge>
          <Badge color="default" size="sm">
            {template.trigger_type}
          </Badge>
          <Badge color={toneColor} size="sm">
            {template.tone}
          </Badge>
          <Badge color="default" size="sm">
            {template.locale}
          </Badge>
        </div>
      </CardHeader>
      <CardContent>
        <div className="flex items-start gap-2">
          <MessageSquare className={cn(iconSize.sm, 'mt-0.5 shrink-0 text-content-muted')} aria-hidden="true" />
          <p className={cn('text-sm leading-relaxed', colors.text.secondary, !expanded && isLong && 'line-clamp-2')}>
            <TemplateText text={template.text} />
          </p>
        </div>
        {isLong && (
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className={cn(
              'mt-2 flex items-center gap-1 text-xs',
              colors.text.tertiary,
              motion.colors,
              'hover:text-content-strong',
            )}
            aria-label={expanded ? t('common.less', 'Show less') : t('common.more', 'Show more')}
          >
            {expanded ? (
              <>
                <ChevronUp className={iconSize.xs} aria-hidden="true" />
                {t('common.less', 'Show less')}
              </>
            ) : (
              <>
                <ChevronDown className={iconSize.xs} aria-hidden="true" />
                {t('common.more')}
              </>
            )}
          </button>
        )}
      </CardContent>
    </Card>
  )
}

// ── Preset Card ──────────────────────────────────────────────

interface PresetCardProps {
  preset: PresetSummaryDto
}

function PresetCard({ preset }: PresetCardProps) {
  const { t } = useTranslation()
  const catColor: BadgeColor = categoryBadgeColor[preset.category] ?? 'default'

  return (
    <Card variant="default" padding="sm">
      <CardHeader>
        <div className="flex items-center justify-between gap-2">
          <CardTitle className={cn(typography.body, typography.weight.semibold, colors.text.primary, 'text-sm')}>
            {preset.name}
          </CardTitle>
          <div className="flex shrink-0 items-center gap-1.5">
            {preset.builtin && (
              <Badge color="info" size="sm">
                {t('playbooks.builtin')}
              </Badge>
            )}
            <Badge color={catColor} size="sm">
              {preset.category}
            </Badge>
          </div>
        </div>
      </CardHeader>
      <CardContent>
        <div className="flex items-start gap-2">
          <Layers className={cn(iconSize.sm, 'mt-0.5 shrink-0 text-content-muted')} aria-hidden="true" />
          <p className={cn('text-sm leading-relaxed', colors.text.secondary)}>{preset.description}</p>
        </div>
        <p className={cn('mt-2 text-xs', colors.text.tertiary)}>{t('playbooks.steps', { count: preset.step_count })}</p>
      </CardContent>
    </Card>
  )
}

// ── Main Page ────────────────────────────────────────────────

export default function Playbooks() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [tab, setTab] = useState<PlaybookTab>('coaching')

  // Coaching filters
  const [profileFilter, setProfileFilter] = useState<string>('')
  const [triggerFilter, setTriggerFilter] = useState<string>('')
  const [localeFilter, setLocaleFilter] = useState<string>('')

  // Preset filters
  const [categoryFilter, setCategoryFilter] = useState<string>('')

  const resetCoachingFilters = () => {
    setProfileFilter('')
    setTriggerFilter('')
    setLocaleFilter('')
  }
  const resetPresetFilters = () => setCategoryFilter('')

  const { data: coachingData, isLoading: coachingLoading } = useQuery({
    queryKey: ['playbooksCoaching'],
    queryFn: fetchCoachingTemplates,
    staleTime: 60_000,
  })

  const { data: presetData, isLoading: presetLoading } = useQuery({
    queryKey: ['playbooksPresets'],
    queryFn: fetchPresetLibrary,
    staleTime: 60_000,
  })

  // Derived filter options
  const templates = coachingData?.templates ?? []
  const presets = presetData?.presets ?? []

  const profileOptions = useMemo(() => {
    const values = [...new Set(templates.map((t) => t.profile))]
    return [{ label: t('playbooks.filterProfile'), value: '' }, ...values.map((v) => ({ label: v, value: v }))]
  }, [templates, t])

  const triggerOptions = useMemo(() => {
    const values = [...new Set(templates.map((t) => t.trigger_type))]
    return [{ label: t('playbooks.filterTrigger'), value: '' }, ...values.map((v) => ({ label: v, value: v }))]
  }, [templates, t])

  const localeOptions = useMemo(() => {
    const values = [...new Set(templates.map((t) => t.locale))]
    return [{ label: t('common.all'), value: '' }, ...values.map((v) => ({ label: v.toUpperCase(), value: v }))]
  }, [templates, t])

  const categoryOptions = useMemo(() => {
    const values = [...new Set(presets.map((p) => p.category))]
    return [{ label: t('playbooks.filterCategory'), value: '' }, ...values.map((v) => ({ label: v, value: v }))]
  }, [presets, t])

  // Filtered lists
  const filteredTemplates = useMemo(
    () =>
      templates.filter(
        (tpl) =>
          (!profileFilter || tpl.profile === profileFilter) &&
          (!triggerFilter || tpl.trigger_type === triggerFilter) &&
          (!localeFilter || tpl.locale === localeFilter),
      ),
    [templates, profileFilter, triggerFilter, localeFilter],
  )

  const filteredPresets = useMemo(
    () => presets.filter((p) => !categoryFilter || p.category === categoryFilter),
    [presets, categoryFilter],
  )

  const tabClass = (active: boolean) =>
    cn(
      'px-4 py-2 text-sm rounded-t',
      typography.weight.medium,
      motion.colors,
      active
        ? 'bg-surface-elevated text-content-strong border-b-2 border-brand-signal'
        : 'text-content-muted hover:text-content-strong',
    )

  return (
    <div className="min-h-full p-6">
      <h1 className={cn(typography.h1, colors.text.pageTitle, 'mb-1')}>{t('playbooks.title')}</h1>
      <p className={cn('mb-6 text-sm', colors.text.secondary)}>
        {tab === 'coaching'
          ? t('playbooks.templateCount', { count: filteredTemplates.length })
          : t('playbooks.templateCount', { count: filteredPresets.length })}
      </p>

      {/* Tabs */}
      <div className="mb-4 flex gap-1 border-muted border-b">
        <button type="button" className={tabClass(tab === 'coaching')} onClick={() => setTab('coaching')}>
          <span className="flex items-center gap-1.5">
            <MessageSquare className="h-3.5 w-3.5" aria-hidden="true" />
            {t('playbooks.coaching')}
          </span>
        </button>
        <button type="button" className={tabClass(tab === 'presets')} onClick={() => setTab('presets')}>
          <span className="flex items-center gap-1.5">
            <BookOpen className="h-3.5 w-3.5" aria-hidden="true" />
            {t('playbooks.presets')}
          </span>
        </button>
      </div>

      {/* Coaching Templates Tab */}
      {tab === 'coaching' && (
        <>
          {/* Filters */}
          <div className="mb-4 flex flex-wrap gap-3">
            <Select
              value={profileFilter}
              onChange={(e) => setProfileFilter(e.target.value)}
              aria-label={t('playbooks.filterProfile')}
              className="min-w-[140px]"
            >
              {profileOptions.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {opt.label}
                </option>
              ))}
            </Select>
            <Select
              value={triggerFilter}
              onChange={(e) => setTriggerFilter(e.target.value)}
              aria-label={t('playbooks.filterTrigger')}
              className="min-w-[140px]"
            >
              {triggerOptions.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {opt.label}
                </option>
              ))}
            </Select>
            <Select
              value={localeFilter}
              onChange={(e) => setLocaleFilter(e.target.value)}
              aria-label={t('common.all')}
              className="min-w-[80px]"
            >
              {localeOptions.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {opt.label}
                </option>
              ))}
            </Select>
          </div>

          {/* Content */}
          {coachingLoading ? (
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              <ListSkeleton rows={6} />
            </div>
          ) : templates.length === 0 ? (
            <GuidanceEmptyState
              icon={<MessageSquare className="h-8 w-8" aria-hidden="true" />}
              title={t('emptyState.playbooksCoaching.title')}
              description={t('emptyState.playbooksCoaching.description')}
              guidance={[
                {
                  icon: <Filter className={iconSize.base} aria-hidden="true" />,
                  title: t('emptyState.playbooksCoaching.guideTriggerTitle'),
                  description: t('emptyState.playbooksCoaching.guideTriggerDescription'),
                },
                {
                  icon: <Settings2 className={iconSize.base} aria-hidden="true" />,
                  title: t('emptyState.playbooksCoaching.guideVariablesTitle'),
                  description: t('emptyState.playbooksCoaching.guideVariablesDescription'),
                },
                {
                  icon: <MessageSquare className={iconSize.base} aria-hidden="true" />,
                  title: t('emptyState.playbooksCoaching.guidePreviewTitle'),
                  description: t('emptyState.playbooksCoaching.guidePreviewDescription'),
                },
              ]}
              primaryAction={{
                label: t('emptyState.playbooksCoaching.action'),
                onClick: () => navigate('/settings/coaching'),
              }}
            />
          ) : filteredTemplates.length === 0 ? (
            <EmptyState
              icon={<MessageSquare className="h-8 w-8" aria-hidden="true" />}
              title={t('emptyState.playbooksCoaching.title')}
              description={t('emptyState.filterEmpty')}
              action={{ label: t('emptyState.clearFilters'), onClick: resetCoachingFilters }}
            />
          ) : (
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {filteredTemplates.map((tpl) => (
                <CoachingCard key={`${tpl.profile}-${tpl.trigger_type}-${tpl.tone}-${tpl.locale}`} template={tpl} />
              ))}
            </div>
          )}
        </>
      )}

      {/* Automation Presets Tab */}
      {tab === 'presets' && (
        <>
          {/* Filters */}
          <div className="mb-4 flex flex-wrap gap-3">
            <Select
              value={categoryFilter}
              onChange={(e) => setCategoryFilter(e.target.value)}
              aria-label={t('playbooks.filterCategory')}
              className="min-w-[140px]"
            >
              {categoryOptions.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {opt.label}
                </option>
              ))}
            </Select>
          </div>

          {/* Content */}
          {presetLoading ? (
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              <ListSkeleton rows={6} />
            </div>
          ) : presets.length === 0 ? (
            <GuidanceEmptyState
              icon={<BookOpen className="h-8 w-8" aria-hidden="true" />}
              title={t('emptyState.playbooksPresets.title')}
              description={t('emptyState.playbooksPresets.description')}
              guidance={[
                {
                  icon: <Settings2 className={iconSize.base} aria-hidden="true" />,
                  title: t('emptyState.playbooksPresets.guideReviewTitle'),
                  description: t('emptyState.playbooksPresets.guideReviewDescription'),
                },
                {
                  icon: <ShieldCheck className={iconSize.base} aria-hidden="true" />,
                  title: t('emptyState.playbooksPresets.guidePolicyTitle'),
                  description: t('emptyState.playbooksPresets.guidePolicyDescription'),
                },
                {
                  icon: <PlayCircle className={iconSize.base} aria-hidden="true" />,
                  title: t('emptyState.playbooksPresets.guideRunTitle'),
                  description: t('emptyState.playbooksPresets.guideRunDescription'),
                },
              ]}
              primaryAction={{
                label: t('emptyState.playbooksPresets.action'),
                onClick: () => navigate('/automation/policies'),
              }}
            />
          ) : filteredPresets.length === 0 ? (
            <EmptyState
              icon={<BookOpen className="h-8 w-8" aria-hidden="true" />}
              title={t('emptyState.playbooksPresets.title')}
              description={t('emptyState.filterEmpty')}
              action={{ label: t('emptyState.clearFilters'), onClick: resetPresetFilters }}
            />
          ) : (
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {filteredPresets.map((preset) => (
                <PresetCard key={preset.id} preset={preset} />
              ))}
            </div>
          )}
        </>
      )}
    </div>
  )
}
