import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { CheckCircle2, Pencil, Plus, Shield, Trash2 } from 'lucide-react'
import { useCallback, useId, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  createExecutionPolicy,
  deleteExecutionPolicy,
  type ExecutionPolicyConfig,
  fetchExecutionPolicies,
  updateExecutionPolicy,
} from '../../api/client'
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Checkbox,
  FieldHint,
  GuidanceEmptyState,
  Input,
  ListSkeleton,
  Select,
  SettingPreview,
} from '../../components/ui'
import { addToast } from '../../hooks/useToast'
import { colors, iconSize, interaction, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

const CONFIRMATION_OPTIONS = ['Auto', 'Confirm', 'Block']
const AUDIT_LEVEL_OPTIONS = ['None', 'Basic', 'Detailed']
const SANDBOX_PROFILE_OPTIONS = ['', 'Permissive', 'Standard', 'Strict']
const POLICY_STARTER_ITEMS = [
  {
    titleKey: 'policies.emptyStepProcessTitle',
    descKey: 'policies.emptyStepProcessDesc',
  },
  {
    titleKey: 'policies.emptyStepConfirmTitle',
    descKey: 'policies.emptyStepConfirmDesc',
  },
  {
    titleKey: 'policies.emptyStepReviewTitle',
    descKey: 'policies.emptyStepReviewDesc',
  },
] as const

type TFunction = ReturnType<typeof useTranslation>['t']

function emptyPolicy(): ExecutionPolicyConfig {
  return {
    policy_id: '',
    process_name: '',
    process_hash: null,
    allowed_args: [],
    requires_sudo: false,
    max_execution_time_ms: 5000,
    audit_level: 'Basic',
    sandbox_profile: null,
    allowed_paths: [],
    allow_network: null,
    require_signed_token: false,
    confirmation: 'Confirm',
  }
}

function splitAllowedArgs(value: string): string[] {
  return value
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
}

function confirmationBadgeColor(value: string) {
  switch (value) {
    case 'Auto':
      return 'success' as const
    case 'Block':
      return 'error' as const
    default:
      return 'warning' as const
  }
}

function confirmationPreviewLabel(value: string, t: TFunction) {
  switch (value) {
    case 'Auto':
      return t('policies.previewAuto', 'Runs without confirmation')
    case 'Block':
      return t('policies.previewBlock', 'Blocked until changed')
    default:
      return t('policies.previewConfirm', 'Confirmation required')
  }
}

interface PolicyStarterEmptyStateProps {
  onAdd: () => void
}

function PolicyStarterEmptyState({ onAdd }: PolicyStarterEmptyStateProps) {
  const { t } = useTranslation()

  return (
    <GuidanceEmptyState
      icon={<Shield className="h-8 w-8" aria-hidden="true" />}
      title={t('policies.emptyTitle', 'No execution policies')}
      description={t(
        'policies.emptyDescription',
        'No commands are approved yet. Start with one local process, keep confirmation on, and review the first run before widening access.',
      )}
      guidance={POLICY_STARTER_ITEMS.map((item) => ({
        title: t(item.titleKey),
        description: t(item.descKey),
        icon: <CheckCircle2 className={iconSize.base} aria-hidden="true" />,
      }))}
      primaryAction={{ label: t('policies.addPolicy', 'Add Policy'), onClick: onAdd }}
    />
  )
}

interface PolicyFormProps {
  initial: ExecutionPolicyConfig
  onSubmit: (policy: ExecutionPolicyConfig) => void
  onCancel: () => void
  isSubmitting: boolean
  isEdit: boolean
}

function PolicyForm({ initial, onSubmit, onCancel, isSubmitting, isEdit }: PolicyFormProps) {
  const { t } = useTranslation()
  const [form, setForm] = useState<ExecutionPolicyConfig>(initial)
  const [argsText, setArgsText] = useState(initial.allowed_args.join('\n'))
  const hintBaseId = useId()
  const policyIdHintId = `${hintBaseId}-policy-id-hint`
  const processNameHintId = `${hintBaseId}-process-name-hint`
  const allowedArgsHintId = `${hintBaseId}-allowed-args-hint`
  const allowedArgs = splitAllowedArgs(argsText)
  const previewRows = [
    {
      label: t('policies.processName', 'Process Name'),
      value: form.process_name || t('policies.previewMissingProcess', 'Choose a process'),
    },
    {
      label: t('policies.confirmation', 'Confirmation'),
      value: confirmationPreviewLabel(form.confirmation, t),
    },
    {
      label: t('policies.auditLevel', 'Audit Level'),
      value: form.audit_level,
    },
    {
      label: t('policies.allowedArgsShort', 'Arguments'),
      value:
        allowedArgs.length > 0
          ? t('policies.previewArgsCount', { count: allowedArgs.length })
          : t('policies.previewArgsNone', 'Arguments: none yet'),
    },
  ]

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    onSubmit({ ...form, allowed_args: allowedArgs })
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4">
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,1fr)_18rem]">
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <div>
              <label htmlFor="policy-id" className={cn('mb-1 block text-sm', typography.weight.medium, 'text-content')}>
                {t('policies.policyId', 'Policy ID')}
              </label>
              <Input
                id="policy-id"
                aria-describedby={policyIdHintId}
                value={form.policy_id}
                onChange={(e) => setForm((f) => ({ ...f, policy_id: e.target.value }))}
                placeholder="pol-git-status"
                required
                disabled={isEdit}
              />
              <FieldHint id={policyIdHintId}>{t('policies.policyIdHint')}</FieldHint>
            </div>
            <div>
              <label
                htmlFor="policy-process-name"
                className={cn('mb-1 block text-sm', typography.weight.medium, 'text-content')}
              >
                {t('policies.processName', 'Process Name')}
              </label>
              <Input
                id="policy-process-name"
                aria-describedby={processNameHintId}
                value={form.process_name}
                onChange={(e) => setForm((f) => ({ ...f, process_name: e.target.value }))}
                placeholder="git"
                required
              />
              <FieldHint id={processNameHintId}>{t('policies.processNameHint')}</FieldHint>
            </div>
            <div>
              <label
                htmlFor="policy-confirmation"
                className={cn('mb-1 block text-sm', typography.weight.medium, 'text-content')}
              >
                {t('policies.confirmation', 'Confirmation')}
              </label>
              <Select
                id="policy-confirmation"
                value={form.confirmation}
                onChange={(e) => setForm((f) => ({ ...f, confirmation: e.target.value }))}
              >
                {CONFIRMATION_OPTIONS.map((opt) => (
                  <option key={opt} value={opt}>
                    {opt}
                  </option>
                ))}
              </Select>
            </div>
            <div>
              <label
                htmlFor="policy-audit-level"
                className={cn('mb-1 block text-sm', typography.weight.medium, 'text-content')}
              >
                {t('policies.auditLevel', 'Audit Level')}
              </label>
              <Select
                id="policy-audit-level"
                value={form.audit_level}
                onChange={(e) => setForm((f) => ({ ...f, audit_level: e.target.value }))}
              >
                {AUDIT_LEVEL_OPTIONS.map((opt) => (
                  <option key={opt} value={opt}>
                    {opt}
                  </option>
                ))}
              </Select>
            </div>
            <div>
              <label
                htmlFor="policy-sandbox-profile"
                className={cn('mb-1 block text-sm', typography.weight.medium, 'text-content')}
              >
                {t('policies.sandboxProfile', 'Sandbox Profile')}
              </label>
              <Select
                id="policy-sandbox-profile"
                value={form.sandbox_profile ?? ''}
                onChange={(e) =>
                  setForm((f) => ({
                    ...f,
                    sandbox_profile: e.target.value || null,
                  }))
                }
              >
                {SANDBOX_PROFILE_OPTIONS.map((opt) => (
                  <option key={opt} value={opt}>
                    {opt || t('policies.sandboxNone', '(none)')}
                  </option>
                ))}
              </Select>
            </div>
            <div>
              <label
                htmlFor="policy-max-exec-time"
                className={cn('mb-1 block text-sm', typography.weight.medium, 'text-content')}
              >
                {t('policies.maxExecutionTime', 'Max Execution Time (ms)')}
              </label>
              <Input
                id="policy-max-exec-time"
                type="number"
                min={100}
                value={form.max_execution_time_ms}
                onChange={(e) =>
                  setForm((f) => ({
                    ...f,
                    max_execution_time_ms: Number(e.target.value) || 5000,
                  }))
                }
              />
            </div>
          </div>
          <div>
            <label
              htmlFor="policy-allowed-args"
              className={cn('mb-1 block text-sm', typography.weight.medium, 'text-content')}
            >
              {t('policies.allowedArgs', 'Allowed Arguments (one per line)')}
            </label>
            <textarea
              id="policy-allowed-args"
              aria-describedby={allowedArgsHintId}
              className={cn(
                'w-full rounded-md border border-muted bg-surface px-3 py-2 text-content text-sm',
                interaction.focusRing,
                'min-h-[80px] resize-y',
              )}
              value={argsText}
              onChange={(e) => setArgsText(e.target.value)}
              placeholder={`--verbose\nstatus\n--*.txt`}
            />
            <FieldHint id={allowedArgsHintId}>{t('policies.allowedArgsHint')}</FieldHint>
          </div>
          <div className="flex flex-wrap items-center gap-3">
            <Checkbox
              checked={form.require_signed_token}
              onChange={(e) => setForm((f) => ({ ...f, require_signed_token: e.target.checked }))}
              label={t('policies.requireSignedToken', 'Require Signed Token')}
            />
            <Checkbox
              checked={form.requires_sudo}
              onChange={(e) => setForm((f) => ({ ...f, requires_sudo: e.target.checked }))}
              label={t('policies.requiresSudo', 'Requires Sudo')}
            />
          </div>
        </div>
        <SettingPreview title={t('policies.previewTitle')} rows={previewRows} footer={t('policies.previewHint')} />
      </div>
      <div className="flex justify-end gap-2 pt-2">
        <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
          {t('common.cancel', 'Cancel')}
        </Button>
        <Button type="submit" variant="primary" size="sm" isLoading={isSubmitting}>
          {isEdit ? t('common.save', 'Save') : t('common.create', 'Create')}
        </Button>
      </div>
    </form>
  )
}

function Policies() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [showForm, setShowForm] = useState(false)
  const [editPolicy, setEditPolicy] = useState<ExecutionPolicyConfig | null>(null)

  const { data: policies, isLoading } = useQuery({
    queryKey: ['executionPolicies'],
    queryFn: fetchExecutionPolicies,
  })

  const createMutation = useMutation({
    mutationFn: createExecutionPolicy,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['executionPolicies'] })
      setShowForm(false)
      addToast('success', t('policies.created', 'Policy created'))
    },
    onError: (err) => {
      addToast('error', err instanceof Error ? err.message : t('policies.createError', 'Failed to create policy'))
    },
  })

  const updateMutation = useMutation({
    mutationFn: (policy: ExecutionPolicyConfig) => updateExecutionPolicy(policy.policy_id, policy),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['executionPolicies'] })
      setEditPolicy(null)
      addToast('success', t('policies.updated', 'Policy updated'))
    },
    onError: (err) => {
      addToast('error', err instanceof Error ? err.message : t('policies.updateError', 'Failed to update policy'))
    },
  })

  const deleteMutation = useMutation({
    mutationFn: deleteExecutionPolicy,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['executionPolicies'] })
      addToast('success', t('policies.deleted', 'Policy deleted'))
    },
    onError: (err) => {
      addToast('error', err instanceof Error ? err.message : t('policies.deleteError', 'Failed to delete policy'))
    },
  })

  const handleCreate = useCallback(
    (policy: ExecutionPolicyConfig) => {
      createMutation.mutate(policy)
    },
    [createMutation],
  )

  const handleUpdate = useCallback(
    (policy: ExecutionPolicyConfig) => {
      updateMutation.mutate(policy)
    },
    [updateMutation],
  )

  if (isLoading) {
    return (
      <div className="min-h-full space-y-6 p-6">
        <div className="h-8 w-48 animate-pulse rounded bg-hover" />
        <ListSkeleton rows={5} />
      </div>
    )
  }

  const hasNoPolicies = !policies || policies.length === 0
  const openCreateForm = () => setShowForm(true)

  return (
    <div className="min-h-full space-y-6 p-6">
      <div className="flex items-center justify-between">
        <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('policies.title', 'Execution Policies')}</h1>
        {!hasNoPolicies && !showForm && !editPolicy && (
          <Button variant="primary" size="sm" onClick={openCreateForm}>
            <Plus className={cn('mr-1.5', iconSize.base)} />
            {t('policies.addPolicy', 'Add Policy')}
          </Button>
        )}
      </div>

      {showForm && (
        <Card>
          <CardHeader>
            <CardTitle>{t('policies.newPolicy', 'New Execution Policy')}</CardTitle>
          </CardHeader>
          <CardContent>
            <PolicyForm
              initial={emptyPolicy()}
              onSubmit={handleCreate}
              onCancel={() => setShowForm(false)}
              isSubmitting={createMutation.isPending}
              isEdit={false}
            />
          </CardContent>
        </Card>
      )}

      {editPolicy && (
        <Card>
          <CardHeader>
            <CardTitle>
              {t('policies.editPolicy', 'Edit Policy')}: {editPolicy.policy_id}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <PolicyForm
              initial={editPolicy}
              onSubmit={handleUpdate}
              onCancel={() => setEditPolicy(null)}
              isSubmitting={updateMutation.isPending}
              isEdit
            />
          </CardContent>
        </Card>
      )}

      {hasNoPolicies && !showForm ? (
        <PolicyStarterEmptyState onAdd={openCreateForm} />
      ) : !hasNoPolicies ? (
        <Card>
          <CardHeader>
            <CardTitle>
              {t('policies.configuredPolicies', 'Configured Policies')} ({policies?.length ?? 0})
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-muted border-b">
                    <th className={cn('px-3 py-2 text-left', typography.weight.medium, 'text-content-secondary')}>
                      {t('policies.policyId', 'Policy ID')}
                    </th>
                    <th className={cn('px-3 py-2 text-left', typography.weight.medium, 'text-content-secondary')}>
                      {t('policies.processName', 'Process Name')}
                    </th>
                    <th className={cn('px-3 py-2 text-left', typography.weight.medium, 'text-content-secondary')}>
                      {t('policies.confirmation', 'Confirmation')}
                    </th>
                    <th className={cn('px-3 py-2 text-left', typography.weight.medium, 'text-content-secondary')}>
                      {t('policies.auditLevel', 'Audit Level')}
                    </th>
                    <th className={cn('px-3 py-2 text-left', typography.weight.medium, 'text-content-secondary')}>
                      {t('policies.sandboxProfile', 'Sandbox')}
                    </th>
                    <th className={cn('px-3 py-2 text-right', typography.weight.medium, 'text-content-secondary')}>
                      {t('policies.maxExecMs', 'Max (ms)')}
                    </th>
                    <th className={cn('px-3 py-2 text-right', typography.weight.medium, 'text-content-secondary')}>
                      {t('common.actions', 'Actions')}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {(policies ?? []).map((policy) => (
                    <tr key={policy.policy_id} className={cn('border-muted border-b', motion.colors)}>
                      <td className={cn('px-3 py-2', typography.family.mono, 'text-content-strong text-xs')}>
                        {policy.policy_id}
                      </td>
                      <td className="px-3 py-2 text-content-strong">{policy.process_name}</td>
                      <td className="px-3 py-2">
                        <Badge color={confirmationBadgeColor(policy.confirmation)} size="sm">
                          {policy.confirmation}
                        </Badge>
                      </td>
                      <td className="px-3 py-2 text-content-strong">{policy.audit_level}</td>
                      <td className="px-3 py-2 text-content-strong">{policy.sandbox_profile || '-'}</td>
                      <td className="px-3 py-2 text-right text-content-strong">{policy.max_execution_time_ms}</td>
                      <td className="px-3 py-2 text-right">
                        <div className="flex justify-end gap-1">
                          <button
                            type="button"
                            onClick={() => setEditPolicy(policy)}
                            className={cn(
                              'rounded p-1 text-content-secondary hover:bg-hover hover:text-content',
                              interaction.focusRing,
                            )}
                            aria-label={t('common.edit', 'Edit')}
                          >
                            <Pencil className={iconSize.base} />
                          </button>
                          <button
                            type="button"
                            onClick={() => deleteMutation.mutate(policy.policy_id)}
                            className={cn(
                              'rounded p-1 text-semantic-error hover:bg-semantic-error/10',
                              interaction.focusRing,
                            )}
                            aria-label={t('common.delete', 'Delete')}
                          >
                            <Trash2 className={iconSize.base} />
                          </button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </CardContent>
        </Card>
      ) : null}
    </div>
  )
}

export default Policies
