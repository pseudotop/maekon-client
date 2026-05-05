use serde::{Deserialize, Serialize};

use super::automation::AutomationAction;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutomationIntent {
    ClickElement {
        text: Option<String>,
        role: Option<String>,
        app_name: Option<String>,
        button: String,
    },
    TypeIntoElement {
        element_text: Option<String>,
        role: Option<String>,
        text: String,
    },
    ExecuteHotkey {
        keys: Vec<String>,
    },
    WaitForText {
        text: String,
        timeout_ms: u64,
    },
    ActivateApp {
        app_name: String,
    },
    Raw(AutomationAction),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiElement {
    pub text: String,
    pub bounds: ElementBounds,
    pub role: Option<String>,
    pub confidence: f64,
    pub source: FinderSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl ElementBounds {
    pub fn center(&self) -> (i32, i32) {
        (
            self.x + self.width as i32 / 2,
            self.y + self.height as i32 / 2,
        )
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinderSource {
    Ocr,
    Accessibility,
    TemplateMatcher,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentResult {
    pub success: bool,
    pub element: Option<UiElement>,
    pub verification: Option<VerificationResult>,
    pub retry_count: u32,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub screen_changed: bool,
    pub changed_regions: usize,
    pub text_found: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentCommand {
    pub command_id: String,
    pub session_id: String,
    pub intent: AutomationIntent,
    pub config: Option<IntentConfig>,
    pub timeout_ms: Option<u64>,
    pub policy_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentConfig {
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_retry_interval_ms")]
    pub retry_interval_ms: u64,
    #[serde(default = "default_verify")]
    pub verify_after_action: bool,
    #[serde(default = "default_verify_delay_ms")]
    pub verify_delay_ms: u64,
}

impl Default for IntentConfig {
    fn default() -> Self {
        Self {
            min_confidence: default_min_confidence(),
            max_retries: default_max_retries(),
            retry_interval_ms: default_retry_interval_ms(),
            verify_after_action: default_verify(),
            verify_delay_ms: default_verify_delay_ms(),
        }
    }
}

fn default_min_confidence() -> f64 {
    0.7
}
fn default_max_retries() -> u32 {
    3
}
fn default_retry_interval_ms() -> u64 {
    500
}
fn default_verify() -> bool {
    true
}
fn default_verify_delay_ms() -> u64 {
    1000
}

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
pub enum PresetCategory {
    Productivity,
    AppManagement,
    Workflow,
    Custom,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_click_element_serde() {
        let intent = AutomationIntent::ClickElement {
            text: Some("save".to_string()),
            role: Some("button".to_string()),
            app_name: None,
            button: "left".to_string(),
        };
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        match deser {
            AutomationIntent::ClickElement { text, button, .. } => {
                assert_eq!(text.unwrap(), "save");
                assert_eq!(button, "left");
            }
            other => unreachable!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn intent_type_into_element_serde() {
        let intent = AutomationIntent::TypeIntoElement {
            element_text: Some("search".to_string()),
            role: None,
            text: "hello world".to_string(),
        };
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        match deser {
            AutomationIntent::TypeIntoElement { text, .. } => {
                assert_eq!(text, "hello world");
            }
            other => unreachable!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn intent_execute_hotkey_serde() {
        let intent = AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "S".to_string()],
        };
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        match deser {
            AutomationIntent::ExecuteHotkey { keys } => {
                assert_eq!(keys.len(), 2);
            }
            other => unreachable!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn intent_wait_for_text_serde() {
        let intent = AutomationIntent::WaitForText {
            text: "completed".to_string(),
            timeout_ms: 5000,
        };
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        match deser {
            AutomationIntent::WaitForText { text, timeout_ms } => {
                assert_eq!(text, "completed");
                assert_eq!(timeout_ms, 5000);
            }
            other => unreachable!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn intent_raw_serde() {
        let intent = AutomationIntent::Raw(AutomationAction::MouseClick {
            button: "left".to_string(),
            x: 100,
            y: 200,
        });
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        assert!(matches!(deser, AutomationIntent::Raw(_)));
    }

    #[test]
    fn intent_activate_app_serde() {
        let intent = AutomationIntent::ActivateApp {
            app_name: "Visual Studio Code".to_string(),
        };
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        match deser {
            AutomationIntent::ActivateApp { app_name } => {
                assert_eq!(app_name, "Visual Studio Code");
            }
            other => unreachable!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn element_bounds_center() {
        let bounds = ElementBounds {
            x: 100,
            y: 200,
            width: 80,
            height: 40,
        };
        let (cx, cy) = bounds.center();
        assert_eq!(cx, 140);
        assert_eq!(cy, 220);
    }

    #[test]
    fn element_bounds_contains() {
        let bounds = ElementBounds {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        assert!(bounds.contains(10, 20));
        assert!(bounds.contains(50, 40));
        assert!(!bounds.contains(110, 20));
        assert!(!bounds.contains(10, 70));
        assert!(!bounds.contains(9, 20));
    }

    #[test]
    fn ui_element_serde() {
        let elem = UiElement {
            text: "save".to_string(),
            bounds: ElementBounds {
                x: 100,
                y: 200,
                width: 80,
                height: 30,
            },
            role: Some("button".to_string()),
            confidence: 0.95,
            source: FinderSource::Ocr,
        };
        let json = serde_json::to_string(&elem).unwrap();
        let deser: UiElement = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.text, "save");
        assert_eq!(deser.confidence, 0.95);
        assert_eq!(deser.source, FinderSource::Ocr);
    }

    #[test]
    fn intent_config_defaults() {
        let config = IntentConfig::default();
        assert!((config.min_confidence - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.retry_interval_ms, 500);
        assert!(config.verify_after_action);
        assert_eq!(config.verify_delay_ms, 1000);
    }

    #[test]
    fn intent_config_serde_with_defaults() {
        let json = "{}";
        let config: IntentConfig = serde_json::from_str(json).unwrap();
        assert!((config.min_confidence - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn intent_config_serde_override() {
        let json = r#"{"min_confidence": 0.9, "max_retries": 5}"#;
        let config: IntentConfig = serde_json::from_str(json).unwrap();
        assert!((config.min_confidence - 0.9).abs() < f64::EPSILON);
        assert_eq!(config.max_retries, 5);
    }

    #[test]
    fn intent_command_serde() {
        let cmd = IntentCommand {
            command_id: "cmd-1".to_string(),
            session_id: "sess-1".to_string(),
            intent: AutomationIntent::ExecuteHotkey {
                keys: vec!["Ctrl".to_string(), "C".to_string()],
            },
            config: None,
            timeout_ms: Some(10000),
            policy_token: "token-abc".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: IntentCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.command_id, "cmd-1");
        assert_eq!(deser.policy_token, "token-abc");
    }

    #[test]
    fn verification_result_serde() {
        let result = VerificationResult {
            screen_changed: true,
            changed_regions: 3,
            text_found: Some(true),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: VerificationResult = serde_json::from_str(&json).unwrap();
        assert!(deser.screen_changed);
        assert_eq!(deser.changed_regions, 3);
    }

    #[test]
    fn intent_result_serde() {
        let result = IntentResult {
            success: true,
            element: None,
            verification: Some(VerificationResult {
                screen_changed: true,
                changed_regions: 1,
                text_found: None,
            }),
            retry_count: 0,
            elapsed_ms: 250,
            error: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: IntentResult = serde_json::from_str(&json).unwrap();
        assert!(deser.success);
        assert_eq!(deser.elapsed_ms, 250);
    }

    #[test]
    fn finder_source_serde() {
        let source = FinderSource::Accessibility;
        let json = serde_json::to_string(&source).unwrap();
        let deser: FinderSource = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, FinderSource::Accessibility);
    }

    #[test]
    fn workflow_preset_serde() {
        let preset = WorkflowPreset {
            id: "save-file".to_string(),
            name: "file save".to_string(),
            description: "Save the current file.".to_string(),
            category: PresetCategory::Productivity,
            steps: vec![WorkflowStep {
                name: "Ctrl+S".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Ctrl".to_string(), "S".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: true,
            }],
            builtin: true,
            platform: None,
            ai_profile_id: None,
        };
        let json = serde_json::to_string(&preset).unwrap();
        let deser: WorkflowPreset = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.id, "save-file");
        assert_eq!(deser.category, PresetCategory::Productivity);
        assert_eq!(deser.steps.len(), 1);
        assert!(deser.builtin);
    }

    #[test]
    fn workflow_step_defaults() {
        let json = r#"{"name":"step1","intent":{"ExecuteHotkey":{"keys":["Ctrl","Z"]}}}"#;
        let step: WorkflowStep = serde_json::from_str(json).unwrap();
        assert_eq!(step.delay_ms, 0);
        assert!(step.stop_on_failure);
    }

    #[test]
    fn preset_category_serde() {
        for cat in [
            PresetCategory::Productivity,
            PresetCategory::AppManagement,
            PresetCategory::Workflow,
            PresetCategory::Custom,
        ] {
            let json = serde_json::to_string(&cat).unwrap();
            let deser: PresetCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(deser, cat);
        }
    }

    #[test]
    fn builtin_presets_load() {
        let presets = builtin_presets();
        assert_eq!(presets.len(), 15);
        assert!(presets.iter().all(|p| p.builtin));
    }

    #[test]
    fn platform_modifier_keys() {
        let m = platform_modifier();
        if cfg!(target_os = "macos") {
            assert_eq!(m, "Cmd");
        } else {
            assert_eq!(m, "Ctrl");
        }
    }

    #[test]
    fn all_presets_have_steps() {
        let presets = builtin_presets();
        for preset in &presets {
            assert!(
                !preset.steps.is_empty(),
                "프리셋 '{}'에 단계 none",
                preset.id
            );
        }
    }

    #[test]
    fn preset_ids_unique() {
        let presets = builtin_presets();
        let ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
        let mut unique_ids = ids.clone();
        unique_ids.sort();
        unique_ids.dedup();
        assert_eq!(ids.len(), unique_ids.len(), "Duplicate preset ID found");
    }

    #[test]
    fn preset_categories_coverage() {
        let presets = builtin_presets();
        let has_productivity = presets
            .iter()
            .any(|p| p.category == PresetCategory::Productivity);
        let has_app = presets
            .iter()
            .any(|p| p.category == PresetCategory::AppManagement);
        let has_workflow = presets
            .iter()
            .any(|p| p.category == PresetCategory::Workflow);
        assert!(has_productivity);
        assert!(has_app);
        assert!(has_workflow);
    }
}
