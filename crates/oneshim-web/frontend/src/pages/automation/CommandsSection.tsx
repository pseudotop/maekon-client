import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { CheckCircle2, ChevronDown, ChevronUp, Clock, ListChecks, Rows3, ShieldCheck, XCircle } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import {
  deletePreset,
  fetchPresets,
  fetchSettings,
  type PresetRunResult,
  runPreset,
  type SavedAiProviderProfile,
  updatePreset,
  type WorkflowPreset,
} from '../../api/client'
import { GuidanceEmptyState, Select } from '../../components/ui'
import { Badge } from '../../components/ui/Badge'
import { Button } from '../../components/ui/Button'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { addToast } from '../../hooks/useToast'
import { useTypedOutletContext } from '../../routes'
import { iconSize, interaction, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { AutomationContext } from './AutomationLayout'

type PresetTab = 'Productivity' | 'AppManagement' | 'Workflow' | 'Custom'

interface RunFeedback {
  presetId: string
  result: PresetRunResult
  timestamp: number
}

function resolvePresetProfileLabel(
  t: ReturnType<typeof useTranslation>['t'],
  preset: WorkflowPreset,
  savedProfiles: SavedAiProviderProfile[],
): string {
  if (!preset.ai_profile_id) {
    return t('automation.profileUsesActive')
  }

  const profile = savedProfiles.find((item) => item.profile_id === preset.ai_profile_id)
  if (!profile) {
    return t('automation.profileMissing', { profileId: preset.ai_profile_id })
  }

  return profile.name
}

export default function CommandsSection() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { status } = useTypedOutletContext<AutomationContext>('Automation')
  const [presetTab, setPresetTab] = useState<PresetTab>('Productivity')
  const [runningPreset, setRunningPreset] = useState<string | null>(null)
  const [runFeedbacks, setRunFeedbacks] = useState<RunFeedback[]>([])
  const [expandedPreset, setExpandedPreset] = useState<string | null>(null)

  const { data: presetsData } = useQuery({
    queryKey: ['presets'],
    queryFn: fetchPresets,
  })

  const { data: settingsData } = useQuery({
    queryKey: ['settings'],
    queryFn: fetchSettings,
  })

  const runPresetMutation = useMutation({
    mutationFn: runPreset,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['auditLogs'] })
      queryClient.invalidateQueries({ queryKey: ['automationStats'] })
    },
  })

  const deletePresetMutation = useMutation({
    mutationFn: deletePreset,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['presets'] })
    },
  })

  const updatePresetMutation = useMutation({
    mutationFn: ({ id, preset }: { id: string; preset: WorkflowPreset }) => updatePreset(id, preset),
    onSuccess: (_preset, variables) => {
      queryClient.invalidateQueries({ queryKey: ['presets'] })
      addToast('success', t('automation.profileBindingSaved', { name: variables.preset.name }))
    },
    onError: (error) => {
      const message = error instanceof Error ? error.message : t('automation.profileBindingSaveError')
      addToast('error', message)
    },
  })

  const handleRunPreset = async (id: string) => {
    setRunningPreset(id)
    setRunFeedbacks((prev) => prev.filter((f) => f.presetId !== id))
    try {
      const result = await runPresetMutation.mutateAsync(id)
      setRunFeedbacks((prev) => [
        ...prev.filter((f) => f.presetId !== id),
        { presetId: id, result, timestamp: Date.now() },
      ])
      addToast(result.success ? 'success' : 'error', result.message)
      setTimeout(() => {
        setRunFeedbacks((prev) => prev.filter((f) => f.presetId !== id))
      }, 8000)
    } catch (error) {
      const message = error instanceof Error ? error.message : t('automation.runError')
      addToast('error', message)
      setRunFeedbacks((prev) => [
        ...prev.filter((f) => f.presetId !== id),
        {
          presetId: id,
          result: { preset_id: id, success: false, message },
          timestamp: Date.now(),
        },
      ])
    } finally {
      setRunningPreset(null)
    }
  }

  const getFeedback = (presetId: string): RunFeedback | undefined => runFeedbacks.find((f) => f.presetId === presetId)
  const savedAiProfiles = settingsData?.ai_provider.saved_profiles ?? []

  const handlePresetProfileChange = async (preset: WorkflowPreset, profileId: string) => {
    try {
      await updatePresetMutation.mutateAsync({
        id: preset.id,
        preset: {
          ...preset,
          ai_profile_id: profileId.trim().length > 0 ? profileId : null,
        },
      })
    } catch {
      // handled by mutation onError
    }
  }

  const filteredPresets = (presetsData?.presets ?? []).filter((p: WorkflowPreset) => p.category === presetTab)

  return (
    <Card id="section-commands">
      <CardHeader>
        <CardTitle>{t('automation.presets')}</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="mb-4 flex space-x-2" role="tablist" aria-label={t('automation.presets')}>
          {(['Productivity', 'AppManagement', 'Workflow', 'Custom'] as PresetTab[]).map((tab) => (
            <button
              type="button"
              key={tab}
              id={`tab-${tab.toLowerCase()}`}
              data-testid={`tab-${tab.toLowerCase()}`}
              role="tab"
              aria-selected={presetTab === tab}
              aria-controls={`tabpanel-${tab.toLowerCase()}`}
              onClick={() => setPresetTab(tab)}
              className={cn(
                `rounded-md px-3 py-1.5 ${typography.weight.medium} text-sm ${motion.colors}`,
                interaction.focusRing,
                presetTab === tab ? 'bg-brand-signal/10 text-brand-text' : 'text-content-secondary hover:bg-hover',
              )}
            >
              {t(`automation.category.${tab}`)}
            </button>
          ))}
        </div>
        <div
          id={`tabpanel-${presetTab.toLowerCase()}`}
          data-testid={`tabpanel-${presetTab.toLowerCase()}`}
          role="tabpanel"
          aria-labelledby={`tab-${presetTab.toLowerCase()}`}
          className="grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-3"
        >
          {filteredPresets.map((preset: WorkflowPreset) => {
            const feedback = getFeedback(preset.id)
            const isExpanded = expandedPreset === preset.id
            return (
              <div
                key={preset.id}
                className={`flex flex-col rounded-lg border p-4 ${motion.colors} ${
                  feedback
                    ? feedback.result.success
                      ? 'border-status-connected bg-semantic-success/10'
                      : 'border-status-error bg-semantic-error/10'
                    : 'border-muted'
                }`}
              >
                <div className="flex items-start justify-between">
                  <div className="min-w-0 flex-1">
                    <h3 className={`${typography.weight.medium} text-content`}>{preset.name}</h3>
                    <p className="mt-1 text-content-secondary text-sm">{preset.description}</p>
                  </div>
                  <div className="ml-2 flex shrink-0 items-center space-x-1">
                    {preset.platform && (
                      <Badge color="default" size="sm">
                        {preset.platform}
                      </Badge>
                    )}
                    {preset.builtin && (
                      <Badge color="info" size="sm">
                        {t('automation.builtin')}
                      </Badge>
                    )}
                  </div>
                </div>

                {/* Steps toggle */}
                <button
                  type="button"
                  onClick={() => setExpandedPreset(isExpanded ? null : preset.id)}
                  className={cn(
                    `mt-2 flex items-center text-content-muted text-xs ${motion.colors} hover:text-content-strong`,
                    interaction.focusRing,
                  )}
                >
                  <span>
                    {preset.steps.length} {t('automation.steps')}
                  </span>
                  {preset.steps.length > 0 &&
                    (isExpanded ? (
                      <ChevronUp className={`ml-1 ${iconSize.xs}`} />
                    ) : (
                      <ChevronDown className={`ml-1 ${iconSize.xs}`} />
                    ))}
                </button>

                {/* Steps list */}
                {isExpanded && preset.steps.length > 0 && (
                  <div className="mt-2 space-y-1">
                    {preset.steps.map((step, idx) => (
                      <div key={step.name} className="flex items-center text-content-secondary text-xs">
                        <span
                          className={`mr-2 flex ${iconSize.base} shrink-0 items-center justify-center rounded-full bg-hover ${typography.weight.medium} text-[10px]`}
                        >
                          {idx + 1}
                        </span>
                        <span className="truncate">{step.name}</span>
                        {step.delay_ms > 0 && (
                          <span className="ml-auto shrink-0 text-content-muted">+{step.delay_ms}ms</span>
                        )}
                      </div>
                    ))}
                  </div>
                )}

                {/* Run feedback */}
                {feedback && (
                  <div
                    className={`mt-3 rounded-md p-2 text-xs ${
                      feedback.result.success
                        ? 'bg-semantic-success/20 text-semantic-success'
                        : 'bg-semantic-error/20 text-semantic-error'
                    }`}
                  >
                    <div className="flex items-center">
                      {feedback.result.success ? (
                        <CheckCircle2 className="mr-1.5 h-3.5 w-3.5 shrink-0" />
                      ) : (
                        <XCircle className="mr-1.5 h-3.5 w-3.5 shrink-0" />
                      )}
                      <span className={`truncate ${typography.weight.medium}`}>{feedback.result.message}</span>
                    </div>
                    {(feedback.result.steps_executed != null || feedback.result.total_elapsed_ms != null) && (
                      <div className="mt-1 ml-6 flex items-center space-x-3 text-[11px] opacity-80">
                        {feedback.result.steps_executed != null && feedback.result.total_steps != null && (
                          <span>
                            {feedback.result.steps_executed}/{feedback.result.total_steps} {t('automation.steps')}
                          </span>
                        )}
                        {feedback.result.total_elapsed_ms != null && (
                          <span className="flex items-center">
                            <Clock className={`mr-0.5 ${iconSize.xs}`} />
                            {feedback.result.total_elapsed_ms}ms
                          </span>
                        )}
                      </div>
                    )}
                  </div>
                )}

                <div className="mt-3 space-y-2 rounded-md border border-muted/80 bg-surface-subtle/60 p-3">
                  <div className="flex items-center justify-between gap-3">
                    <span className="text-content-secondary text-xs">{t('automation.aiProfile')}</span>
                    <Badge color="default" size="sm">
                      {resolvePresetProfileLabel(t, preset, savedAiProfiles)}
                    </Badge>
                  </div>
                  {!preset.builtin ? (
                    savedAiProfiles.length > 0 ? (
                      <Select
                        value={preset.ai_profile_id ?? ''}
                        selectSize="sm"
                        onChange={(e) => void handlePresetProfileChange(preset, e.target.value)}
                        disabled={updatePresetMutation.isPending}
                      >
                        <option value="">{t('automation.useActiveProfile')}</option>
                        {savedAiProfiles.map((profile) => (
                          <option key={profile.profile_id} value={profile.profile_id}>
                            {profile.name}
                          </option>
                        ))}
                      </Select>
                    ) : (
                      <div className="space-y-1 text-xs">
                        <p className="text-content-secondary">{t('automation.noSavedAiProfiles')}</p>
                        <button
                          type="button"
                          onClick={() => navigate('/settings/ai-automation')}
                          className={cn('text-brand-text underline-offset-2 hover:underline', interaction.focusRing)}
                        >
                          {t('automation.manageAiProfiles')}
                        </button>
                      </div>
                    )
                  ) : (
                    <p className="text-content-muted text-xs">{t('automation.builtinProfileBindingReadonly')}</p>
                  )}
                </div>

                <div className="mt-auto flex items-center space-x-2 pt-3">
                  <Button
                    data-testid={`run-preset-${preset.id}`}
                    variant="primary"
                    size="sm"
                    isLoading={runningPreset === preset.id}
                    onClick={() => handleRunPreset(preset.id)}
                    disabled={!status?.enabled}
                  >
                    {t('automation.run')}
                  </Button>
                  {!preset.builtin && (
                    <Button
                      data-testid={`delete-preset-${preset.id}`}
                      variant="danger"
                      size="sm"
                      onClick={() => deletePresetMutation.mutate(preset.id)}
                    >
                      {t('common.delete')}
                    </Button>
                  )}
                </div>
              </div>
            )
          })}
          {filteredPresets.length === 0 && (
            <GuidanceEmptyState
              className="col-span-full py-8"
              icon={<ListChecks className={iconSize.lg} aria-hidden="true" />}
              title={t('automation.noPresets')}
              description={t('automation.noPresetsDescription')}
              guidance={[
                {
                  icon: <Rows3 className={iconSize.sm} aria-hidden="true" />,
                  title: t('automation.noPresetsGuideCategoryTitle'),
                  description: t('automation.noPresetsGuideCategoryDescription'),
                },
                {
                  icon: <CheckCircle2 className={iconSize.sm} aria-hidden="true" />,
                  title: t('automation.noPresetsGuideProfilesTitle'),
                  description: t('automation.noPresetsGuideProfilesDescription'),
                },
                {
                  icon: <ShieldCheck className={iconSize.sm} aria-hidden="true" />,
                  title: t('automation.noPresetsGuidePolicyTitle'),
                  description: t('automation.noPresetsGuidePolicyDescription'),
                },
              ]}
            />
          )}
        </div>
      </CardContent>
    </Card>
  )
}
