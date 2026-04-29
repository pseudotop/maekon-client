import { useQuery } from '@tanstack/react-query'
import { Bot, ClipboardCheck, History, ShieldCheck } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { fetchAutomationContracts, fetchPolicies } from '../../api/client'
import { GuidanceEmptyState } from '../../components/ui'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { useTypedOutletContext } from '../../routes'
import { iconSize, typography } from '../../styles/tokens'
import type { AutomationContext } from './AutomationLayout'

export default function PoliciesSection() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { status, stats } = useTypedOutletContext<AutomationContext>('Automation')

  const { data: policies } = useQuery({
    queryKey: ['policies'],
    queryFn: fetchPolicies,
  })

  const { data: contracts } = useQuery({
    queryKey: ['automationContracts'],
    queryFn: fetchAutomationContracts,
  })

  // Automation-wide empty state — owned here (not AutomationLayout) so the
  // layout can always render <Outlet> and `/automation` → `/automation/policies`
  // keeps redirecting. Same AuditLayout empty-state-in-child pattern.
  if ((stats?.total_executions ?? 0) === 0 && !status?.enabled) {
    return (
      <GuidanceEmptyState
        icon={<Bot className="h-8 w-8" />}
        title={t('emptyState.automation.title')}
        description={t('emptyState.automation.description')}
        guidance={[
          {
            icon: <ShieldCheck className={iconSize.sm} aria-hidden="true" />,
            title: t('emptyState.automation.guideEnableTitle'),
            description: t('emptyState.automation.guideEnableDescription'),
          },
          {
            icon: <ClipboardCheck className={iconSize.sm} aria-hidden="true" />,
            title: t('emptyState.automation.guidePolicyTitle'),
            description: t('emptyState.automation.guidePolicyDescription'),
          },
          {
            icon: <History className={iconSize.sm} aria-hidden="true" />,
            title: t('emptyState.automation.guideAuditTitle'),
            description: t('emptyState.automation.guideAuditDescription'),
          },
        ]}
        primaryAction={{ label: t('emptyState.automation.action'), onClick: () => navigate('/settings') }}
      />
    )
  }

  return (
    <Card id="section-policies">
      <CardHeader>
        <CardTitle>{t('automation.policies')}</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="grid grid-cols-1 gap-4 text-sm md:grid-cols-2 lg:grid-cols-3">
          <div>
            <div className="text-content-secondary">{t('automation.automationEnabled')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>
              {policies?.automation_enabled ? t('automation.enabled') : t('automation.disabled')}
            </div>
          </div>
          <div>
            <div className="text-content-secondary">{t('automation.sandboxProfile')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>{policies?.sandbox_profile ?? '-'}</div>
          </div>
          <div>
            <div className="text-content-secondary">{t('automation.sandboxEnabled')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>
              {policies?.sandbox_enabled ? t('automation.enabled') : t('automation.disabled')}
            </div>
          </div>
          <div>
            <div className="text-content-secondary">{t('automation.allowNetwork')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>
              {policies?.allow_network ? t('automation.enabled') : t('automation.disabled')}
            </div>
          </div>
          <div>
            <div className="text-content-secondary">{t('automation.dataPolicy')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>
              {policies?.external_data_policy ?? '-'}
            </div>
          </div>
          <div>
            <div className="text-content-secondary">{t('automation.sceneOverride')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>
              {policies?.scene_action_override_active
                ? t('automation.active')
                : policies?.scene_action_override_enabled
                  ? t('automation.pending')
                  : t('automation.disabled')}
            </div>
          </div>
          <div>
            <div className="text-content-secondary">{t('automation.sceneOverrideExpires')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>
              {policies?.scene_action_override_expires_at
                ? new Date(policies.scene_action_override_expires_at).toLocaleString()
                : '-'}
            </div>
          </div>
          <div>
            <div className="text-content-secondary">{t('automation.sceneOverrideIssue')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>
              {policies?.scene_action_override_issue || '-'}
            </div>
          </div>
          <div>
            <div className="text-content-secondary">{t('automation.sceneSchemaVersion')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>
              {contracts?.scene_schema_version ?? '-'}
            </div>
          </div>
          <div>
            <div className="text-content-secondary">{t('automation.auditSchemaVersion')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>
              {contracts?.audit_schema_version ?? '-'}
            </div>
          </div>
          <div>
            <div className="text-content-secondary">{t('automation.sceneActionSchemaVersion')}</div>
            <div className={`mt-1 ${typography.weight.medium} text-content`}>
              {contracts?.scene_action_schema_version ?? '-'}
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
  )
}
