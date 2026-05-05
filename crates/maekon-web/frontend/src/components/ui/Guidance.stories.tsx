import type { Meta, StoryObj } from '@storybook/react'
import { CheckCircle2, Info, Shield } from 'lucide-react'
import { Button } from './Button'
import { FieldHint, GuidanceEmptyState, GuidancePanel, SettingPreview, UnavailableFeatureCallout } from './Guidance'
import { Input } from './Input'

const meta = {
  title: 'UI Primitives/Guidance',
  tags: ['autodocs'],
} satisfies Meta

export default meta
type Story = StoryObj<typeof meta>

export const EmptyState: Story = {
  render: () => (
    <GuidanceEmptyState
      icon={<Shield className="h-8 w-8" />}
      title="No execution policies"
      description="Start with one trusted local process, keep confirmation on, and review the first run."
      guidance={[
        {
          icon: <CheckCircle2 className="h-4 w-4" />,
          title: 'Choose one process',
          description: 'Use an exact command name such as git.',
        },
        {
          icon: <CheckCircle2 className="h-4 w-4" />,
          title: 'Keep confirmation on',
          description: 'Require approval until the first run looks correct.',
        },
        {
          icon: <CheckCircle2 className="h-4 w-4" />,
          title: 'Review the run',
          description: 'Check execution history before widening access.',
        },
      ]}
      primaryAction={{ label: 'Add Policy', onClick: () => {} }}
      secondaryAction={{ label: 'Read Guide', onClick: () => {} }}
    />
  ),
}

export const FormGuidance: Story = {
  render: () => (
    <div className="grid max-w-4xl grid-cols-[minmax(0,1fr)_18rem] gap-6">
      <div>
        <label htmlFor="policy-id-story" className="mb-1 block font-medium text-content text-sm">
          Policy ID
        </label>
        <Input id="policy-id-story" aria-describedby="policy-id-story-hint" placeholder="pol-git-status" />
        <FieldHint id="policy-id-story-hint">Stable internal id, for example pol-git-status.</FieldHint>
      </div>
      <SettingPreview
        title="Policy preview"
        rows={[
          { label: 'Process', value: 'git' },
          { label: 'Confirmation', value: 'Confirmation required', tone: 'warning' },
          { label: 'Audit level', value: 'Basic' },
        ]}
        footer="Default to confirmation until the first execution looks correct."
      />
    </div>
  ),
}

export const PageGuidance: Story = {
  render: () => (
    <div className="max-w-4xl">
      <GuidancePanel
        title="Before changing settings"
        description="Orient the user before a dense settings surface asks for decisions."
        items={[
          {
            icon: <Info className="h-4 w-4" />,
            title: 'Understand the current state',
            description: 'Show what this page controls before exposing low-level inputs.',
          },
          {
            icon: <CheckCircle2 className="h-4 w-4" />,
            title: 'Make one change',
            description: 'Encourage smaller edits so runtime impact is easier to verify.',
          },
          {
            icon: <Shield className="h-4 w-4" />,
            title: 'Review privacy impact',
            description: 'Call out when settings affect capture, export, network, or retention behavior.',
          },
        ]}
      />
    </div>
  ),
}

export const UnavailableFeature: Story = {
  render: () => (
    <div className="max-w-xl">
      <UnavailableFeatureCallout
        icon={<Info className="h-5 w-5" />}
        title="Nightly updates are disabled"
        description="Nightly artifacts are not supported in this build stream."
        reason="Choose Stable or Pre-release for now."
        action={{ label: 'Open Updates', onClick: () => {} }}
      />
    </div>
  ),
}

export const HintTones: Story = {
  render: () => (
    <div className="max-w-md space-y-3">
      <FieldHint>Default helper copy for a normal setting.</FieldHint>
      <FieldHint tone="info">This setting affects local-only behavior.</FieldHint>
      <FieldHint tone="warning">Changing this may require a restart.</FieldHint>
      <FieldHint tone="danger">This can remove local data.</FieldHint>
      <Button type="button" variant="primary">
        Save
      </Button>
    </div>
  ),
}
