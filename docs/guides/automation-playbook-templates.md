[English](./automation-playbook-templates.md) | [한국어](./automation-playbook-templates.ko.md)

# Automation Playbook Templates

This guide maps built-in workflow presets to practical day-to-day usage.

## How to use

1. Open `/automation` in the local dashboard.
2. Select `Workflow` category.
3. Run a built-in preset and observe results in Audit Log + KPI cards.
4. Clone or adapt as custom presets for your team.

## Built-in templates (recommended start order)

| Preset ID | When to use | Expected outcome |
|---|---|---|
| `daily-priority-sync` | Start of workday | Calendar/issue/chat context aligned in under 1 minute |
| `bug-triage-loop` | Bug queue handling | Faster context switching between tracker/monitoring/IDE |
| `customer-followup` | Customer response windows | CRM-doc-mail flow standardized |
| `release-readiness` | Before release validation | Save + terminal + browser loop starts consistently |
| `deep-work-start` | Focus sessions | Workspace narrowed for execution |

## Operational guardrails

- Keep sandbox enabled for repeatable policy boundaries.
- Use `scene_action_override` only for time-bound exceptions.
- Track `success_rate`, `blocked_rate`, and `p95_elapsed_ms` in Automation KPI cards.

## Team rollout tip

Start with 2-3 templates that are already repeated manually. Add more only after KPI trend improves for one week.
