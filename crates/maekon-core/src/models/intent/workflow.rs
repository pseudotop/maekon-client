//! Workflow presets: `WorkflowPreset`, `WorkflowStep`, `PresetCategory`,
//! and `builtin_presets()`.

use serde::{Deserialize, Serialize};

use super::automation::AutomationIntent;

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
    presets.push(WorkflowPreset {
        id: "morning-routine".to_string(),
        name: "Start of Day".to_string(),
        description: "Launch Mail, Calendar, and VS Code to begin your workday".to_string(),
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
        description: "Open Zoom and Notes to get ready for a meeting".to_string(),
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
        description: "Open Calendar, Jira, and Slack in sequence to align on the day's priorities"
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
                name: "Open Jira".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Jira".to_string(),
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
        description: "Cycle through the issue tracker, monitoring tools, and IDE to triage bugs"
            .to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Open Issue Tracker".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Issue Tracker".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Open Monitoring".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Monitoring".to_string(),
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
            "Open CRM, Notion, and Mail to review customer feedback and prepare follow-up actions"
                .to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Open CRM".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "CRM".to_string(),
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
        description: "Save code, then open Terminal and a browser to kick off release checks"
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

    presets.push(WorkflowPreset {
        id: "deep-work-start".to_string(),
        name: "Start Deep Work".to_string(),
        description: "Open VS Code and dismiss distractions to begin a focused work session"
            .to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Open VS Code".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Visual Studio Code".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: true,
            },
            WorkflowStep {
                name: "Switch to next app".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![alt.to_string(), "Tab".to_string()],
                },
                delay_ms: 800,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Close current window".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![m.to_string(), "W".to_string()],
                },
                delay_ms: 500,
                stop_on_failure: false,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets
}
