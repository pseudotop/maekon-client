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
        name: "file save".to_string(),
        description: "current file을 save합니다".to_string(),
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
        name: "execution 취소".to_string(),
        description: "마지막 작업을 execution 취소합니다".to_string(),
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
        name: "전체 선택 후 복사".to_string(),
        description: "전체 선택(Ctrl+A) 후 복사(Ctrl+C)를 execution합니다".to_string(),
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
        name: "찾기/바꾸기".to_string(),
        description: "찾기/바꾸기 대화상자를 엽니다".to_string(),
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
        name: "next 앱 전환".to_string(),
        description: "next 애플리케이션으로 전환합니다".to_string(),
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
        name: "current 창 닫기".to_string(),
        description: "current active 창을 닫습니다".to_string(),
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
            name: "전체 최소화".to_string(),
            description: "all 창을 최소화합니다".to_string(),
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
            name: "전체 최소화".to_string(),
            description: "all 창을 최소화합니다".to_string(),
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

    presets.push(WorkflowPreset {
        id: "morning-routine".to_string(),
        name: "업무 started".to_string(),
        description: "Mail → Calendar → VSCode 순으로 앱을 active화합니다".to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Mail 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Mail".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Calendar 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Calendar".to_string(),
                },
                delay_ms: 2000,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "VSCode 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Visual Studio Code".to_string(),
                },
                delay_ms: 2000,
                stop_on_failure: false,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "meeting-prep".to_string(),
        name: "회의 준비".to_string(),
        description: "Zoom과 Notes를 열어 회의를 준비합니다".to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Zoom 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Zoom".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Notes 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Notes".to_string(),
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
        id: "end-of-day".to_string(),
        name: "업무 ended".to_string(),
        description: "file save 후 앱을 ended합니다".to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "file save".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![m.to_string(), "S".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "앱 ended".to_string(),
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
        name: "우선순위 점검".to_string(),
        description: "캘린더, 이슈 보드, 메신저를 순서대로 열어 당일 우선순위를 정리합니다"
            .to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Calendar 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Calendar".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Jira 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Jira".to_string(),
                },
                delay_ms: 1200,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Slack 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Slack".to_string(),
                },
                delay_ms: 1200,
                stop_on_failure: false,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "bug-triage-loop".to_string(),
        name: "버그 트리아지".to_string(),
        description: "이슈 트래커, 로그/모니터링, IDE를 순환하며 버그를 정리합니다".to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "Issue Tracker 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Issue Tracker".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Monitoring 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Monitoring".to_string(),
                },
                delay_ms: 1200,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "VSCode 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Visual Studio Code".to_string(),
                },
                delay_ms: 1200,
                stop_on_failure: false,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    });

    presets.push(WorkflowPreset {
        id: "customer-followup".to_string(),
        name: "고객 팔로업".to_string(),
        description: "고객 feedback 확인 후 문서와 메일을 열어 후속 조치를 준비합니다".to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "CRM 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "CRM".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Notion 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Notion".to_string(),
                },
                delay_ms: 1000,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Mail 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Mail".to_string(),
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
        id: "release-readiness".to_string(),
        name: "릴리스 준비".to_string(),
        description: "코드 save 후 터미널과 브라우저를 열어 릴리스 체크를 started합니다"
            .to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "file save".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![m.to_string(), "S".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Terminal 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Terminal".to_string(),
                },
                delay_ms: 500,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Browser 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Google Chrome".to_string(),
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
        id: "deep-work-start".to_string(),
        name: "집중 session started".to_string(),
        description: "IDE를 열고 메신저를 뒤로 보within 집중 session을 started합니다".to_string(),
        category: PresetCategory::Workflow,
        steps: vec![
            WorkflowStep {
                name: "VSCode 열기".to_string(),
                intent: AutomationIntent::ActivateApp {
                    app_name: "Visual Studio Code".to_string(),
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "next 앱 전환".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec![alt.to_string(), "Tab".to_string()],
                },
                delay_ms: 800,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "current 창 닫기".to_string(),
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
