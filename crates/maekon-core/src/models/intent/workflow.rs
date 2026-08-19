//! Workflow presets: `WorkflowPreset`, `WorkflowStep`, `PresetCategory`,
//! and `builtin_presets()`.

use serde::{Deserialize, Serialize};

use super::automation::AutomationIntent;
use crate::models::suggestion::{SuggestionSource, SuggestionType};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: PresetCategory,
    pub steps: Vec<WorkflowStep>,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub ai_profile_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub name: String,
    pub intent: AutomationIntent,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default = "default_stop_on_failure")]
    pub stop_on_failure: bool,
}

fn default_stop_on_failure() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PresetCategory {
    Productivity,
    AppManagement,
    Workflow,
    Custom,
}

// F-RC-C37-03: Display returns PascalCase, matching serde wire contract.
impl std::fmt::Display for PresetCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Productivity => f.write_str("Productivity"),
            Self::AppManagement => f.write_str("AppManagement"),
            Self::Workflow => f.write_str("Workflow"),
            Self::Custom => f.write_str("Custom"),
        }
    }
}

pub fn platform_modifier() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    }
}

pub fn platform_alt_modifier() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Alt"
    }
}

/// Builtin preset id offered as the one-click automation bound to a
/// locally-minted `NeedFocusTime` nudge (T4.1 #7917). Kept next to
/// `builtin_presets()` so the const and the builtin id are edited together; a
/// guard test (`preset_deep_work_start_const_resolves_to_a_builtin`) asserts the
/// const still resolves to a real builtin.
pub const PRESET_DEEP_WORK_START: &str = "deep-work-start";

/// Derived suggestion → automation binding (T4.1 #7917, ADR-027).
///
/// Maps a suggestion to a builtin automation preset id PURELY from its
/// `(type, source)` — there is no persisted binding field and no producer
/// change. Only the locally-minted, rule-based `NeedFocusTime` nudge earns an
/// action; a server- or LLM-sourced suggestion of the SAME type gets `None`, so
/// a network-pushed suggestion can never gain a promptless execution affordance
/// (ADR-027 frozen invariant: "network-sourced suggestions gain no execution
/// affordance"). The `source` arm is load-bearing: `NeedFocusTime` is
/// server-mintable over the SSE/gRPC wire, and LLM-authored content paired with
/// an execution affordance is prompt-injection-adjacent even for a fixed preset.
///
/// This is the single predicate shared by both DTO build sites and the
/// run-command server-side revalidation, so all three stay consistent by
/// construction.
pub fn suggested_action_preset(
    suggestion_type: &SuggestionType,
    source: &SuggestionSource,
) -> Option<&'static str> {
    match (suggestion_type, source) {
        (SuggestionType::NeedFocusTime, SuggestionSource::RuleBased) => {
            Some(PRESET_DEEP_WORK_START)
        }
        _ => None,
    }
}

pub fn builtin_presets() -> Vec<WorkflowPreset> {
    let m = platform_modifier();
    let alt = platform_alt_modifier();

    let mut presets = Vec::new();

    presets.push(WorkflowPreset {
        id: "save-file".to_string(),
        name: "Save File".to_string(),
        description: "Save the current file".to_string(),
        category: PresetCategory::Productivity,
        steps: vec![WorkflowStep {
            name: format!("{}+S", m),
            intent: AutomationIntent::ExecuteHotkey {
                keys: vec![m.to_string(), "S".to_string()],
            },
            delay_ms: 0,
            stop_on_failure: true,
        }],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "undo".to_string(),
        name: "Undo".to_string(),
        description: "Undo the last action".to_string(),
        category: PresetCategory::Productivity,
        steps: vec![WorkflowStep {
            name: format!("{}+Z", m),
            intent: AutomationIntent::ExecuteHotkey {
                keys: vec![m.to_string(), "Z".to_string()],
            },
            delay_ms: 0,
            stop_on_failure: true,
        }],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "select-all-copy".to_string(),
        name: "Select All and Copy".to_string(),
        description: "Select all content and copy it to the clipboard".to_string(),
        category: PresetCategory::Productivity,
        steps: vec![
            WorkflowStep {
                name: format!("{}+A", m),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![m.to_string(), "A".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: format!("{}+C", m),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![m.to_string(), "C".to_string()],
                },
                delay_ms: 200,
                stop_on_failure: true,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "find-replace".to_string(),
        name: "Find and Replace".to_string(),
        description: "Open the Find and Replace dialog".to_string(),
        category: PresetCategory::Productivity,
        steps: vec![WorkflowStep {
            name: format!("{}+H", m),
            intent: AutomationIntent::ExecuteHotkey {
                keys: vec![m.to_string(), "H".to_string()],
            },
            delay_ms: 0,
            stop_on_failure: true,
        }],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "switch-next-app".to_string(),
        name: "Switch to Next App".to_string(),
        description: "Switch focus to the next application".to_string(),
        category: PresetCategory::AppManagement,
        steps: vec![WorkflowStep {
            name: format!("{}+Tab", alt),
            intent: AutomationIntent::ExecuteHotkey {
                keys: vec![alt.to_string(), "Tab".to_string()],
            },
            delay_ms: 0,
            stop_on_failure: true,
        }],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "close-window".to_string(),
        name: "Close Window".to_string(),
        description: "Close the currently active window".to_string(),
        category: PresetCategory::AppManagement,
        steps: vec![WorkflowStep {
            name: format!("{}+W", m),
            intent: AutomationIntent::ExecuteHotkey {
                keys: vec![m.to_string(), "W".to_string()],
            },
            delay_ms: 0,
            stop_on_failure: true,
        }],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    if cfg!(target_os = "macos") {
        presets.push(WorkflowPreset {
            id: "minimize-all".to_string(),
            name: "Minimize All Windows".to_string(),
            description: "Minimize all open windows".to_string(),
            category: PresetCategory::AppManagement,
            steps: vec![WorkflowStep {
                name: "Cmd+Option+H+M".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![
                        "Cmd".to_string(),
                        "Option".to_string(),
                        "H".to_string(),
                        "M".to_string(),
                    ],
                },
                delay_ms: 0,
                stop_on_failure: false,
            }],
            builtin: true,
            platform: Some("macos".to_string()),
            ai_profile_id: None,
        });
    } else {
        presets.push(WorkflowPreset {
            id: "minimize-all".to_string(),
            name: "Minimize All Windows".to_string(),
            description: "Minimize all open windows".to_string(),
            category: PresetCategory::AppManagement,
            steps: vec![WorkflowStep {
                name: "Win+D".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Win".to_string(), "D".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: false,
            }],
            builtin: true,
            platform: Some("windows".to_string()),
            ai_profile_id: None,
        });
    }

    // MAEKON-AUTO-1 (#7070) interim safety: every builtin ActivateApp step below uses
    // `stop_on_failure: true`. A failed/un-switched activation must HALT the workflow
    // rather than let a subsequent step synthesize input (e.g. Cmd+W / Alt+Tab) against
    // whatever window currently has focus.
    //
    // Cross-platform activation semantics (#8055 P2-2): `ActivateApp` brings a window
    // to the FRONT. Only macOS `open -a <name>` ALSO launches the app when it is not
    // running; on Windows (`AppActivate`) and Linux (`wmctrl`/`xdotool`) the target app
    // must ALREADY be open, otherwise the step reports failure and — per the rule above —
    // halts the workflow. The descriptions below therefore promise "bring to the front"
    // (the honest common behaviour), not "launch". These are environment-specific SAMPLE
    // templates: the named apps must be installed (macOS) or already running (Win/Linux),
    // or the user edits the app names for their setup — see
    // docs/guides/automation-playbook-templates.md ("Platform differences").
    presets.push(WorkflowPreset {
        id: "morning-routine".to_string(),
        name: "Start of Day".to_string(),
        description: "Bring Mail, Calendar, and VS Code to the front to begin your workday"
            .to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Open Mail".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Mail".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open Calendar".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Calendar".to_string(),
                },
                delay_ms: 2000,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open VS Code".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Visual Studio Code".to_string(),
                },
                delay_ms: 2000,
                stop_on_failure: true,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "meeting-prep".to_string(),
        name: "Meeting Prep".to_string(),
        description: "Bring Zoom and Notes to the front to get ready for a meeting".to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Open Zoom".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Zoom".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open Notes".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Notes".to_string(),
                },
                delay_ms: 1000,
                stop_on_failure: true,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "end-of-day".to_string(),
        name: "End of Day".to_string(),
        description: "Save the current file and quit the application".to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Save file".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![m.to_string(), "S".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Quit application".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![m.to_string(), "Q".to_string()],
                },
                delay_ms: 1000,
                stop_on_failure: false,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "daily-priority-sync".to_string(),
        name: "Daily Priority Review".to_string(),
        description:
            "Bring Calendar, Notion, and Slack to the front to align on the day's priorities"
                .to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Open Calendar".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Calendar".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open Notion".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Notion".to_string(),
                },
                delay_ms: 1200,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open Slack".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Slack".to_string(),
                },
                delay_ms: 1200,
                stop_on_failure: true,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "bug-triage-loop".to_string(),
        name: "Bug Triage".to_string(),
        description: "Bring Slack, Terminal, and VS Code to the front to triage incoming bugs"
            .to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Open Slack".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Slack".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open Terminal".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Terminal".to_string(),
                },
                delay_ms: 1200,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open VS Code".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Visual Studio Code".to_string(),
                },
                delay_ms: 1200,
                stop_on_failure: true,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "customer-followup".to_string(),
        name: "Customer Follow-Up".to_string(),
        description:
            "Bring Calendar, Notion, and Mail to the front to review customer feedback and \
             schedule follow-up actions"
                .to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Open Calendar".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Calendar".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open Notion".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Notion".to_string(),
                },
                delay_ms: 1000,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open Mail".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Mail".to_string(),
                },
                delay_ms: 1000,
                stop_on_failure: true,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "release-readiness".to_string(),
        name: "Release Readiness".to_string(),
        description: "Save code, then bring Terminal and a browser to the front to kick off \
             release checks"
            .to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Save file".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![m.to_string(), "S".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Open Terminal".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Terminal".to_string(),
                },
                delay_ms: 500,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open Browser".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Google Chrome".to_string(),
                },
                delay_ms: 1000,
                stop_on_failure: true,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    // Focus-session setup preset offered as the one-click action on a locally-minted
    // NeedFocusTime nudge (T4.1 #7917, ADR-027). Redesigned (#7940) away from the prior
    // developer-centric, partially-destructive steps: it no longer assumes Visual Studio
    // Code is installed (the NeedFocusTime audience is communication-heavy and often has
    // no developer tools, so the marquee button used to error on its first press) and it
    // never blind-closes a window/tab (the old `Alt+Tab` → `Cmd/Ctrl+W` closed whatever
    // the app switch happened to land on). Every step here is app-agnostic and
    // non-destructive:
    //   * `Maekon` is the host app itself, so it exists by construction — activating it
    //     is a guaranteed target, NOT an app-specific assumption about third-party
    //     software. Per MAEKON-AUTO-1 (#7070) the activation that PRECEDES a hotkey keeps
    //     `stop_on_failure: true` so a failed/un-switched activation halts BEFORE the
    //     "Hide Others" hotkey could fire against the wrong front window.
    //   * "Hide Others" (macOS: Cmd+Option+H) / "Show Desktop" (Windows/Linux: Win+D)
    //     only hides or minimizes windows — fully reversible, nothing is closed.
    // The name/description promise only what the steps actually do (honesty rule): they
    // clear the screen so the user can start focusing. They do NOT toggle any OS
    // "Do Not Disturb" or Maekon's own focus mode — that toggle is IPC-only
    // (`toggle_focus_mode`) and unreachable from the AutomationIntent verb set without a
    // new intent verb, which is out of scope for a preset-steps redesign.
    let deep_work_steps = if cfg!(target_os = "macos") {
        // Activate Maekon first (guaranteed target), THEN hide every other app, so the
        // user lands on the Maekon dashboard with a clear screen.
        vec![
            WorkflowStep {
                name: "Open Maekon dashboard".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Maekon".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Hide other apps".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Cmd".to_string(), "Option".to_string(), "H".to_string()],
                },
                delay_ms: 400,
                stop_on_failure: false,
            },
        ]
    } else {
        // No cross-app "hide others" on Windows/Linux. Verify that the host target can
        // be activated BEFORE synthesizing the global Show Desktop shortcut, then
        // minimize everything and bring the verified Maekon dashboard back to the front.
        // Both activations stop on failure, so neither the reversible clearing action nor
        // any later input can continue after a missing/unresolved host target (#8466).
        vec![
            WorkflowStep {
                name: "Verify Maekon dashboard".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Maekon".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Show desktop".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Win".to_string(), "D".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Open Maekon dashboard".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Maekon".to_string(),
                },
                delay_ms: 400,
                stop_on_failure: true,
            },
        ]
    };

    presets.push(WorkflowPreset {
        id: "deep-work-start".to_string(),
        name: "Clear Distractions".to_string(),
        description: "Bring the Maekon dashboard forward and tuck other windows out of \
             the way so you can start a focused work session. Nothing is closed — hidden \
             or minimized windows come right back."
            .to_string(),
        category: PresetCategory::Workflow,
        steps: deep_work_steps,
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets
}

#[cfg(test)]
mod preset_app_name_tests {
    use super::*;

    /// Collect every `ActivateApp` target app name across all builtin presets.
    fn builtin_activate_app_names() -> Vec<String> {
        builtin_presets()
            .iter()
            .flat_map(|preset| preset.steps.iter())
            .filter_map(|step| match &step.intent {
                AutomationIntent::ActivateApp { app_name } => Some(app_name.clone()),
                _ => None,
            })
            .collect()
    }

    /// Guard (#8055 P2-1): no builtin `ActivateApp` step may reference a
    /// placeholder/generic name that resolves to no real application on any
    /// platform. Such ghosts (`open -a "Issue Tracker"` etc.) always exit
    /// non-zero, so a `stop_on_failure` step halts the whole workflow at step 1
    /// ("partially failed 0/N"). Builtin presets must name real, commonly
    /// installed apps; users clone-and-edit for their own tooling.
    #[test]
    fn no_builtin_activate_app_step_uses_a_ghost_placeholder_name() {
        // Names that are category descriptions, not launchable app identifiers.
        const GHOST_NAMES: &[&str] = &["Issue Tracker", "Monitoring", "CRM", "Jira"];
        let names = builtin_activate_app_names();
        assert!(
            !names.is_empty(),
            "expected at least one builtin ActivateApp step to guard"
        );
        for name in &names {
            assert!(
                !GHOST_NAMES.contains(&name.as_str()),
                "builtin preset references ghost app name {name:?} that resolves to no real \
                 application — replace with a real, commonly installed app (#8055 P2-1)"
            );
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn deep_work_preset_verifies_host_before_and_after_show_desktop() {
        let preset = builtin_presets()
            .into_iter()
            .find(|preset| preset.id == PRESET_DEEP_WORK_START)
            .expect("deep-work-start builtin preset");

        assert_eq!(preset.steps.len(), 3);
        for index in [0, 2] {
            assert!(
                matches!(
                    &preset.steps[index].intent,
                    AutomationIntent::ActivateApp { app_name } if app_name == "Maekon"
                ),
                "step {index} must activate the verified Maekon host target"
            );
            assert!(preset.steps[index].stop_on_failure);
        }
        assert!(matches!(
            &preset.steps[1].intent,
            AutomationIntent::ExecuteHotkey { keys }
                if keys == &["Win".to_string(), "D".to_string()]
        ));
    }
}

#[cfg(test)]
mod suggestion_binding_tests {
    use super::*;

    /// Guard: the `PRESET_DEEP_WORK_START` const must resolve to a real builtin
    /// preset id, so the derived binding can never point at a dangling id.
    #[test]
    fn preset_deep_work_start_const_resolves_to_a_builtin() {
        assert!(
            builtin_presets()
                .iter()
                .any(|p| p.id == PRESET_DEEP_WORK_START),
            "PRESET_DEEP_WORK_START must match a real builtin_presets() id"
        );
    }

    /// The MVP binding maps ONLY `(NeedFocusTime, RuleBased)`. Every other
    /// (type, source) pair — crucially the server- and LLM-sourced variants of
    /// the SAME type — resolves to `None`.
    #[test]
    fn suggested_action_preset_maps_only_rulebased_need_focus_time() {
        assert_eq!(
            suggested_action_preset(&SuggestionType::NeedFocusTime, &SuggestionSource::RuleBased),
            Some(PRESET_DEEP_WORK_START),
        );
        // Same type, non-local source ⇒ no affordance (frozen invariant).
        assert_eq!(
            suggested_action_preset(&SuggestionType::NeedFocusTime, &SuggestionSource::LlmServer),
            None,
        );
        assert_eq!(
            suggested_action_preset(&SuggestionType::NeedFocusTime, &SuggestionSource::LlmLocal),
            None,
        );
        // Unmapped types ⇒ None even when locally rule-based.
        assert_eq!(
            suggested_action_preset(&SuggestionType::TakeBreak, &SuggestionSource::RuleBased),
            None,
        );
        assert_eq!(
            suggested_action_preset(&SuggestionType::WorkGuidance, &SuggestionSource::RuleBased),
            None,
        );
    }
}
