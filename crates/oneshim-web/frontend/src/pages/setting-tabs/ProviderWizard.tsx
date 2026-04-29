import anthropicSvg from '@lobehub/icons-static-svg/icons/anthropic.svg?raw'
import copilotSvg from '@lobehub/icons-static-svg/icons/copilot-color.svg?raw'
import deepseekSvg from '@lobehub/icons-static-svg/icons/deepseek-color.svg?raw'
import googleSvg from '@lobehub/icons-static-svg/icons/google-brand-color.svg?raw'
import groqSvg from '@lobehub/icons-static-svg/icons/groq.svg?raw'
import mistralSvg from '@lobehub/icons-static-svg/icons/mistral-color.svg?raw'
import nvidiaSvg from '@lobehub/icons-static-svg/icons/nvidia-color.svg?raw'
import ollamaSvg from '@lobehub/icons-static-svg/icons/ollama.svg?raw'
import openaiSvg from '@lobehub/icons-static-svg/icons/openai.svg?raw'
import openrouterSvg from '@lobehub/icons-static-svg/icons/openrouter.svg?raw'
import xaiSvg from '@lobehub/icons-static-svg/icons/xai.svg?raw'
import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Badge, Button, Card, Input } from '../../components/ui'
import { colors, motion, radius, typography } from '../../styles/tokens'
import type { BadgeColor } from '../../styles/variants'
import { cn } from '../../utils/cn'

const toDataUri = (svg: string) => `data:image/svg+xml,${encodeURIComponent(svg)}`
const anthropicIcon = toDataUri(anthropicSvg)
const copilotIcon = toDataUri(copilotSvg)
const deepseekIcon = toDataUri(deepseekSvg)
const googleIcon = toDataUri(googleSvg)
const groqIcon = toDataUri(groqSvg)
const mistralIcon = toDataUri(mistralSvg)
const nvidiaIcon = toDataUri(nvidiaSvg)
const ollamaIcon = toDataUri(ollamaSvg)
const openaiIcon = toDataUri(openaiSvg)
const openrouterIcon = toDataUri(openrouterSvg)
const xaiIcon = toDataUri(xaiSvg)

interface ProviderDef {
  id: string
  name: string
  icon: string
  tier: 'recommended' | 'free' | 'local' | 'cloud' | 'oauth'
  comingSoon?: boolean
  tierLabel: string
  apiKeyEnv: string
  docsUrl: string
  placeholder: string
  surfaceId: string
  defaultModel: string
}

const PROVIDERS: ProviderDef[] = [
  {
    id: 'anthropic',
    name: 'Claude (Anthropic)',
    icon: anthropicIcon,
    tier: 'recommended',
    tierLabel: 'settings.ai.tierRecommended',
    apiKeyEnv: 'ANTHROPIC_API_KEY',
    docsUrl: 'https://console.anthropic.com/settings/keys',
    placeholder: 'sk-ant-...',
    surfaceId: 'provider_surface.anthropic.direct_api',
    defaultModel: 'claude-sonnet-4-20250514',
  },
  {
    id: 'openai',
    name: 'GPT (OpenAI)',
    icon: openaiIcon,
    tier: 'recommended',
    tierLabel: 'settings.ai.tierRecommended',
    apiKeyEnv: 'OPENAI_API_KEY',
    docsUrl: 'https://platform.openai.com/api-keys',
    placeholder: 'sk-...',
    surfaceId: 'provider_surface.openai.direct_api',
    defaultModel: 'gpt-5.4',
  },
  {
    id: 'groq',
    name: 'Groq',
    icon: groqIcon,
    tier: 'free',
    tierLabel: 'settings.ai.tierFree',
    apiKeyEnv: 'GROQ_API_KEY',
    docsUrl: 'https://console.groq.com/keys',
    placeholder: 'gsk_...',
    surfaceId: 'provider_surface.groq.direct_api',
    defaultModel: 'llama-3.3-70b-versatile',
  },
  {
    id: 'ollama',
    name: 'Ollama',
    icon: ollamaIcon,
    tier: 'local',
    tierLabel: 'settings.ai.tierLocal',
    apiKeyEnv: '',
    docsUrl: 'https://ollama.com/download',
    placeholder: '',
    surfaceId: 'provider_surface.ollama.local_http',
    defaultModel: 'qwen3:8b',
  },
  {
    id: 'google',
    name: 'Gemini (Google)',
    icon: googleIcon,
    tier: 'cloud',
    tierLabel: 'settings.ai.tierCloud',
    apiKeyEnv: 'GOOGLE_API_KEY',
    docsUrl: 'https://aistudio.google.com/apikey',
    placeholder: 'AIza...',
    surfaceId: 'provider_surface.google.direct_api',
    defaultModel: 'gemini-2.5-flash',
  },
  {
    id: 'deepseek',
    name: 'DeepSeek',
    icon: deepseekIcon,
    tier: 'cloud',
    tierLabel: 'settings.ai.tierCloud',
    apiKeyEnv: 'DEEPSEEK_API_KEY',
    docsUrl: 'https://platform.deepseek.com/api_keys',
    placeholder: 'sk-...',
    surfaceId: 'provider_surface.deepseek.direct_api',
    defaultModel: 'deepseek-chat',
  },
  {
    id: 'mistral',
    name: 'Mistral AI',
    icon: mistralIcon,
    tier: 'cloud',
    tierLabel: 'settings.ai.tierCloud',
    apiKeyEnv: 'MISTRAL_API_KEY',
    docsUrl: 'https://console.mistral.ai/api-keys',
    placeholder: '',
    surfaceId: 'provider_surface.mistral.direct_api',
    defaultModel: 'mistral-large-latest',
  },
  {
    id: 'xai',
    name: 'xAI (Grok)',
    icon: xaiIcon,
    tier: 'cloud',
    tierLabel: 'settings.ai.tierCloud',
    apiKeyEnv: 'XAI_API_KEY',
    docsUrl: 'https://console.x.ai/',
    placeholder: 'xai-...',
    surfaceId: 'provider_surface.xai.direct_api',
    defaultModel: 'grok-3',
  },
  {
    id: 'copilot',
    name: 'GitHub Copilot',
    icon: copilotIcon,
    tier: 'oauth',
    tierLabel: 'settings.ai.tierOauth',
    apiKeyEnv: '',
    docsUrl: 'https://github.com/settings/copilot',
    placeholder: '',
    surfaceId: 'provider_surface.copilot.managed_oauth',
    defaultModel: 'gpt-5.4',
    comingSoon: true,
  },
  // Amazon Bedrock removed per ADR-019 — AWS SigV4 unimplemented; re-introduction
  // requires the 8-step checklist in docs/architecture/ADR-019-error-code-infrastructure.md §5.
  {
    id: 'openrouter',
    name: 'OpenRouter',
    icon: openrouterIcon,
    tier: 'cloud',
    tierLabel: 'settings.ai.tierAggregator',
    apiKeyEnv: 'OPENROUTER_API_KEY',
    docsUrl: 'https://openrouter.ai/settings/keys',
    placeholder: 'sk-or-...',
    surfaceId: 'provider_surface.openrouter.direct_api',
    defaultModel: 'anthropic/claude-sonnet-4',
  },
  {
    id: 'nvidia',
    name: 'NVIDIA NIM',
    icon: nvidiaIcon,
    tier: 'cloud',
    tierLabel: 'settings.ai.tierCloud',
    apiKeyEnv: 'NVIDIA_API_KEY',
    docsUrl: 'https://build.nvidia.com/',
    placeholder: 'nvapi-...',
    surfaceId: 'provider_surface.nvidia.direct_api',
    defaultModel: 'nvidia/nemotron-3-super-120b',
  },
]

const TIER_BADGE_COLOR: Record<string, BadgeColor> = {
  recommended: 'success',
  free: 'info',
  local: 'primary',
  cloud: 'info',
  oauth: 'warning',
}

function ProviderIcon({ icon, size = 28 }: { icon: string; size?: number }) {
  return <img src={icon} alt="" width={size} height={size} className="shrink-0" loading="lazy" />
}

interface ProviderWizardProps {
  onSelect: (provider: ProviderDef, apiKey: string) => void
  className?: string
}

export default function ProviderWizard({ onSelect, className }: ProviderWizardProps) {
  const { t } = useTranslation()
  const [selected, setSelected] = useState<ProviderDef | null>(null)
  const [apiKey, setApiKey] = useState('')
  const [testing, setTesting] = useState(false)
  const [testResult, setTestResult] = useState<'idle' | 'success' | 'error'>('idle')
  const [showAll, setShowAll] = useState(false)

  const visibleProviders = showAll ? PROVIDERS : PROVIDERS.slice(0, 6)

  const handleTest = useCallback(async () => {
    if (!selected) return
    setTesting(true)
    setTestResult('idle')
    try {
      const resp = await fetch('/api/ai/providers/models', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          surface_id: selected.surfaceId,
          endpoint: null,
          api_key: apiKey.trim() || null,
        }),
      })
      setTestResult(resp.ok ? 'success' : 'error')
    } catch {
      setTestResult('error')
    } finally {
      setTesting(false)
    }
  }, [selected, apiKey])

  const handleSave = useCallback(() => {
    if (!selected) return
    onSelect(selected, apiKey.trim())
    setSelected(null)
    setApiKey('')
    setTestResult('idle')
  }, [selected, apiKey, onSelect])

  // Step 2: Setup form for selected provider
  if (selected) {
    const needsApiKey = selected.tier !== 'local' && selected.tier !== 'oauth'

    return (
      <Card className={cn('relative', className)}>
        <button
          type="button"
          onClick={() => {
            setSelected(null)
            setApiKey('')
            setTestResult('idle')
          }}
          className={cn(
            'absolute top-4 right-4 px-2 py-1 text-xs',
            radius.sm,
            colors.text.secondary,
            'hover:bg-surface-hover',
          )}
        >
          {t('common.back', 'Back')}
        </button>

        <div className="mb-6 flex items-center gap-3">
          <ProviderIcon icon={selected.icon} size={36} />
          <div>
            <h3 className={typography.h3}>{selected.name}</h3>
            <Badge size="xs" color={TIER_BADGE_COLOR[selected.tier]} className="mt-1">
              {t(selected.tierLabel)}
            </Badge>
          </div>
        </div>

        {needsApiKey && (
          <div className="mb-4">
            <label htmlFor="wizard-api-key" className={cn('mb-1.5 block text-sm', colors.text.secondary)}>
              {t('settings.ai.apiKey', 'API Key')}
            </label>
            <Input
              id="wizard-api-key"
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder={selected.placeholder}
              className={cn('w-full', typography.family.mono)}
            />
            <a
              href={selected.docsUrl}
              target="_blank"
              rel="noopener noreferrer"
              className={cn('mt-1.5 inline-block text-xs', colors.primary.text)}
            >
              {t('settings.ai.getApiKey', 'Get API key')} &rarr;
            </a>
          </div>
        )}

        {selected.tier === 'local' && (
          <div className={cn('mb-4 p-3 text-sm', radius.md, 'bg-surface-hover')}>
            <p className={colors.text.secondary}>
              {t('settings.ai.ollamaRunning', 'Make sure Ollama is running on')}{' '}
              <code className={typography.family.mono}>localhost:11434</code>
            </p>
            <a
              href={selected.docsUrl}
              target="_blank"
              rel="noopener noreferrer"
              className={cn('mt-1 inline-block text-xs', colors.primary.text)}
            >
              {t('settings.ai.downloadOllama', 'Download Ollama')} &rarr;
            </a>
          </div>
        )}

        {selected.tier === 'oauth' && (
          <div className={cn('mb-4 p-3 text-sm', radius.md, 'bg-surface-hover')}>
            <p className={colors.text.secondary}>
              {t(
                'settings.ai.copilotOauth',
                'GitHub Copilot requires OAuth authentication. Connect via the advanced settings below.',
              )}
            </p>
          </div>
        )}

        <div className="mb-4">
          <label htmlFor="wizard-model" className={cn('mb-1.5 block text-sm', colors.text.secondary)}>
            {t('settings.ai.defaultModel', 'Default Model')}
          </label>
          <Input
            id="wizard-model"
            type="text"
            value={selected.defaultModel}
            readOnly
            className={cn('w-full bg-surface-hover', typography.family.mono)}
          />
        </div>

        <div className="flex items-center gap-2">
          {needsApiKey && (
            <Button variant="secondary" onClick={handleTest} disabled={!apiKey || testing} className="text-sm">
              {testing ? t('settings.ai.testing', 'Testing...') : t('settings.ai.testConnection', 'Test Connection')}
            </Button>
          )}

          {testResult === 'success' && (
            <span className={cn('text-xs', colors.semantic.success)}>{t('settings.ai.connected', 'Connected')}</span>
          )}
          {testResult === 'error' && (
            <span className={cn('text-xs', colors.semantic.error)}>
              {t('settings.ai.connectionFailed', 'Connection failed — check your API key')}
            </span>
          )}

          <div className="flex-1" />

          <Button variant="primary" onClick={handleSave} disabled={needsApiKey && !apiKey} className="text-sm">
            {t('settings.ai.saveActivate', 'Save & Activate')}
          </Button>
        </div>
      </Card>
    )
  }

  // Step 1: Provider picker grid
  return (
    <Card className={className}>
      <h3 className={cn(typography.h3, 'mb-1')}>{t('settings.ai.quickSetup', 'Quick Setup')}</h3>
      <p className={cn('mb-4 text-sm', colors.text.secondary)}>
        {t('settings.ai.quickSetupDesc', 'Choose a provider to get started. You can change this anytime.')}
      </p>

      <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
        {visibleProviders.map((provider) => (
          <button
            key={provider.id}
            type="button"
            onClick={() => !provider.comingSoon && setSelected(provider)}
            disabled={provider.comingSoon}
            className={cn(
              'flex items-center gap-2.5 p-3 text-left',
              radius.md,
              'border border-border-default',
              provider.comingSoon ? 'cursor-not-allowed opacity-50' : 'hover:border-brand hover:bg-surface-hover',
              motion.colors,
            )}
          >
            <ProviderIcon icon={provider.icon} />
            <div className="min-w-0">
              <div className={cn('truncate', typography.label, colors.text.primary)}>{provider.name}</div>
              {provider.comingSoon ? (
                <Badge size="xs" className="mt-0.5">
                  {t('settings.ai.comingSoon', 'Coming soon')}
                </Badge>
              ) : (
                <Badge size="xs" color={TIER_BADGE_COLOR[provider.tier]} className="mt-0.5">
                  {t(provider.tierLabel)}
                </Badge>
              )}
            </div>
          </button>
        ))}
      </div>

      {!showAll && PROVIDERS.length > 6 && (
        <button
          type="button"
          onClick={() => setShowAll(true)}
          className={cn('mt-3 text-xs', colors.primary.text, 'hover:underline')}
        >
          {t('settings.ai.showAllProviders', 'Show all {{count}} providers', { count: PROVIDERS.length })} &rarr;
        </button>
      )}
    </Card>
  )
}

export type { ProviderDef }
