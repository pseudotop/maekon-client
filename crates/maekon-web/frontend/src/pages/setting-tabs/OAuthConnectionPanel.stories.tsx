import type { Meta, StoryObj } from '@storybook/react'
import OAuthConnectionPanel from './OAuthConnectionPanel'

const meta = {
  title: 'Settings/OAuthConnectionPanel',
  component: OAuthConnectionPanel,
  tags: ['autodocs'],
} satisfies Meta<typeof OAuthConnectionPanel>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {
  args: {
    providerId: 'google',
    providerName: 'Google',
  },
}

export const WithOAuthSurface: Story = {
  args: {
    providerId: 'github',
    providerName: 'GitHub',
    oauthSurface: {
      id: 'github-oauth',
      display_name: 'GitHub OAuth',
      provider_type: 'github',
      surface_type: 'oauth',
      maturity: 'stable',
      supports_model_discovery: false,
    },
  },
}

export const WithPreferredCli: Story = {
  args: {
    providerId: 'google',
    providerName: 'Google',
    oauthSurface: {
      id: 'google-oauth',
      display_name: 'Google OAuth',
      provider_type: 'google',
      surface_type: 'oauth',
      maturity: 'beta',
      supports_model_discovery: false,
    },
    preferredCliSurface: {
      id: 'google-cli',
      display_name: 'Google CLI (gcloud)',
      provider_type: 'google',
      surface_type: 'cli',
      maturity: 'stable',
      supports_model_discovery: false,
    },
    onUsePreferredCli: () => {},
  },
}
