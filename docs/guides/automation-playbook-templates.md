[English](./automation-playbook-templates.md) | [한국어](./automation-playbook-templates.ko.md)

# Automation Playbook Templates

This guide maps built-in workflow presets to practical day-to-day usage.

## How to use

1. Open `/automation` in the local dashboard.
2. Select `Workflow` category.
3. Run a built-in preset and observe results in Audit Log + KPI cards.
4. Clone or adapt as custom presets for your team.

## Built-in templates (recommended start order)

The `*-sync` / `*-loop` / `*-followup` presets are **samples**: they bring a
specific set of apps to the front. Treat the app list as a starting point and
edit it for the apps you actually use (see **Platform differences** below for
what "bring to the front" means on each OS).

| Preset ID | When to use | Sample flow (edit for your setup) |
|---|---|---|
| `daily-priority-sync` | Start of workday | Bring Calendar, Notion, Slack to the front |
| `bug-triage-loop` | Bug queue handling | Bring Slack, Terminal, VS Code to the front |
| `customer-followup` | Customer response windows | Bring Calendar, Notion, Mail to the front |
| `release-readiness` | Before release validation | Save, then bring Terminal and a browser to the front |
| `deep-work-start` | Focus sessions | Workspace narrowed for execution (app-agnostic) |

## Platform differences (app activation)

The app-switching presets above run `ActivateApp` steps. What that does depends
on the OS, so pick apps and preconditions accordingly:

| Platform | Mechanism | Behavior |
|---|---|---|
| macOS | `open -a "<name>"` | **Brings to the front, and launches the app if it is not already running.** |
| Windows | `WScript.Shell.AppActivate` | Brings an **already-open** window to the front. Does **not** launch — start the app first. |
| Linux | `wmctrl -a` / `xdotool` | Activates an **already-open** window. Does **not** launch — start the app first. Requires `wmctrl` or `xdotool` installed. |

Practical guidance:

- On **macOS**, make sure the named apps are installed. A name that does not
  match a real app (e.g. a generic label) exits non-zero, and because the
  built-in steps use `stop_on_failure`, the whole preset halts at that step.
- On **Windows / Linux**, open the apps you want first (or edit the preset to
  reference apps you keep running), since these presets switch focus rather than
  launch.
- Each `ActivateApp` shell-out is bounded by a short timeout: a wedged
  launch/activation fails the step promptly instead of hanging the workflow.

## Operational guardrails

- Keep sandbox enabled for repeatable policy boundaries.
- Use `scene_action_override` only for time-bound exceptions.
- Track `success_rate`, `blocked_rate`, and `p95_elapsed_ms` in Automation KPI cards.

## Team rollout tip

Start with 2-3 templates that are already repeated manually. Add more only after KPI trend improves for one week.
