//! `AutomationIntent` — the core automation action enum.

use serde::{Deserialize, Serialize};

use crate::models::automation::AutomationAction;

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
