# E19 AI Provider Settings Desktop Smoke

This runbook covers issue #4756. It is a workflow_dispatch/manual only Windows
desktop smoke for the installed Maekon app. It proves that Settings > AI &
Automation renders provider CLI readiness in the real desktop shell.

## Scope

- Open the installed Maekon app in a real Windows desktop session.
- Navigate to Settings > AI & Automation.
- Exercise the LLM provider surface selector with Claude, Codex, and Gemini CLI
  surfaces.
- Record the visible readiness class for each surface: installed, auth-ready,
  invocation-ready, or unavailable.
- Verify Save/Revert does not corrupt provider routing.
- Collect only a redacted screenshot or structured UI snapshot.

## Why Headless Is Not Enough

A headless test cannot cover whether the installed Windows desktop shell renders
the provider selector, CLI readiness badges, floating Save/Revert controls, or
captured evidence exactly as the user sees them. Headless tests remain useful for
contract coverage, but E19 acceptance needs one real desktop observation.

## Required Selectors

- Tab: `[data-testid='settings-ai-automation-tab']`
- Access mode: `[data-testid='settings-ai-access-mode']`
- LLM provider surface: `[data-testid='settings-llm-provider-surface']`
- LLM provider status: `[data-testid='settings-provider-surface-status-llm_api']`
- LLM CLI readiness: `[data-testid='settings-provider-cli-readiness-llm_api']`
- LLM CLI discovery: `[data-testid='settings-provider-cli-discovery-llm_api']`
- LLM active routing: `[data-testid='settings-ai-provider-active-routing-llm']`
- Save: `[data-testid='settings-save']`
- Floating Save: `[data-testid='settings-save-floating']`
- Floating Revert: `[data-testid='settings-revert-floating']`

## Provider Matrix

| Provider | Surface ID | Required evidence |
| --- | --- | --- |
| Claude | `provider_surface.anthropic.subprocess_cli` | Provider row visible, CLI readiness badge visible, no account identifier |
| Codex | `provider_surface.openai.subprocess_cli` | Provider row visible, CLI readiness badge visible, no account identifier |
| Gemini | `provider_surface.google.subprocess_cli` | Provider row visible, CLI readiness badge visible, no account identifier |

Each provider result must map to one of these classes:

- installed
- auth-ready
- invocation-ready
- unavailable

## Save/Revert Routing Check

1. Capture the initial provider routing summary.
2. Switch the LLM provider surface to the next CLI surface.
3. Confirm the floating Save/Revert panel appears.
4. Press Revert.
5. Confirm provider routing matches the initial summary.
6. Repeat with Save only when the profile is disposable or restore proof is
   already captured.

Do not save into a real personal profile unless the run is isolated by VM
snapshot or disposable user profile.

## Privacy Rules

- Use a redacted screenshot or structured UI snapshot.
- Do not record raw account identifiers, organization names, home paths, tokens,
  full environment variables, or provider-specific auth output.
- If a CLI subprocess fails, record only the provider name and the failure class.
- The evidence bundle should classify provider CLI subprocess failure separately
  from UI rendering failure.

## Failure Taxonomy

- settings_navigation_failure
- provider_row_missing
- cli_badge_render_mismatch
- provider_cli_subprocess_failure
- save_revert_routing_corruption
- privacy_redaction_failure

## Acceptance

The run passes only when Settings > AI & Automation shows Claude, Codex, and
Gemini CLI readiness states, Save/Revert preserves provider routing, and the
captured evidence satisfies the redaction rules above.
