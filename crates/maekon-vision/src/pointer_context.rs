use std::collections::BTreeSet;

use maekon_core::models::intent::ElementBounds;
use maekon_core::models::ui_scene::{UiScene, UiSceneElement};
use serde::{Deserialize, Serialize};

use maekon_core::config::PiiFilterLevel;

use crate::privacy::sanitize_title_with_level;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointerContextRequest {
    pub active_app: String,
    pub active_window_title: Option<String>,
    #[serde(default)]
    pub active_process: Option<ActiveProcessWindow>,
    pub cursor_position: Option<(i32, i32)>,
    pub selection_text: Option<String>,
    pub instruction: Option<String>,
    pub scene: UiScene,
    pub evidence_sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveProcessWindow {
    pub process_name: String,
    pub bundle_id: Option<String>,
    pub pid: Option<u32>,
    pub screen_id: Option<String>,
    pub window_title: Option<String>,
    pub window_bounds: ElementBounds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredTargetEntity {
    pub entity_id: String,
    pub kind: String,
    pub label: String,
    pub value: String,
    pub bounds: ElementBounds,
    pub confidence: f64,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointerContextResolution {
    pub active_app: String,
    pub active_window_title: Option<String>,
    pub target: Option<StructuredTargetEntity>,
    pub candidates: Vec<StructuredTargetEntity>,
    pub needs_clarification: bool,
    pub clarification_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointerActionProposal {
    pub trace_id: String,
    pub target_entity_id: String,
    pub target_label: String,
    pub action_kind: String,
    pub highlight_bounds: ElementBounds,
    pub policy_gate_visible: bool,
    pub confirmation_required: bool,
    pub action_not_auto_executed: bool,
    pub allowed_action_payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessGuiSuggestionAnchor {
    pub process_name: String,
    pub bundle_id: Option<String>,
    pub pid: Option<u32>,
    pub screen_id: Option<String>,
    pub window_title: Option<String>,
    pub window_bounds: ElementBounds,
    pub target_entity_id: String,
    pub target_label: String,
    pub target_bounds: ElementBounds,
    pub target_center: (i32, i32),
    pub target_inside_window: bool,
    pub highlight_bounds: ElementBounds,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointerAuditTrace {
    pub trace_id: String,
    pub capture_event_id: String,
    pub pointer_event_id: String,
    pub proposal_event_id: String,
    pub decision_event_id: String,
    pub decision: String,
}

pub fn resolve_pointer_context(request: &PointerContextRequest) -> PointerContextResolution {
    let mut candidates: Vec<_> = request
        .scene
        .elements
        .iter()
        .map(|element| entity_from_element(element, request))
        .collect();

    candidates.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.label.cmp(&b.label))
    });

    let top = candidates.first().cloned();
    let ambiguous = request
        .instruction
        .as_deref()
        .is_some_and(is_deictic_instruction)
        && candidates.get(1).is_some_and(|second| {
            top.as_ref()
                .is_some_and(|first| first.confidence - second.confidence < 0.08)
        });

    PointerContextResolution {
        active_app: request.active_app.clone(),
        active_window_title: request.active_window_title.clone(),
        target: if ambiguous { None } else { top },
        candidates,
        needs_clarification: ambiguous,
        clarification_reason: ambiguous.then_some(
            "Multiple visible targets are plausible for this/that instruction".to_string(),
        ),
    }
}

pub fn build_pointer_action_proposal(
    resolution: &PointerContextResolution,
    action_kind: &str,
) -> Option<PointerActionProposal> {
    let target = resolution.target.as_ref()?;
    let trace_id = maekon_core::generate_id("ptr");
    Some(PointerActionProposal {
        trace_id,
        target_entity_id: target.entity_id.clone(),
        target_label: target.label.clone(),
        action_kind: action_kind.to_string(),
        highlight_bounds: target.bounds.clone(),
        policy_gate_visible: true,
        confirmation_required: true,
        action_not_auto_executed: true,
        allowed_action_payload: serde_json::json!({
            "target_entity_id": target.entity_id,
            "action_kind": action_kind,
            "requires_confirmation": true,
        }),
    })
}

pub fn build_process_gui_suggestion_anchor(
    request: &PointerContextRequest,
    resolution: &PointerContextResolution,
) -> Option<ProcessGuiSuggestionAnchor> {
    let active_process = request.active_process.as_ref()?;
    let target = resolution.target.as_ref()?;
    let target_inside_window =
        bounds_contains_bounds(&active_process.window_bounds, &target.bounds);
    if !target_inside_window {
        return None;
    }

    Some(ProcessGuiSuggestionAnchor {
        process_name: active_process.process_name.clone(),
        bundle_id: active_process.bundle_id.clone(),
        pid: active_process.pid,
        screen_id: active_process
            .screen_id
            .clone()
            .or_else(|| request.scene.screen_id.clone()),
        window_title: active_process
            .window_title
            .clone()
            .or_else(|| request.active_window_title.clone()),
        window_bounds: active_process.window_bounds.clone(),
        target_entity_id: target.entity_id.clone(),
        target_label: target.label.clone(),
        target_bounds: target.bounds.clone(),
        target_center: target.bounds.center(),
        target_inside_window,
        highlight_bounds: target.bounds.clone(),
        sources: target.sources.clone(),
    })
}

pub fn audit_trace_for_decision(
    capture_event_id: &str,
    proposal: &PointerActionProposal,
    decision: &str,
) -> PointerAuditTrace {
    PointerAuditTrace {
        trace_id: proposal.trace_id.clone(),
        capture_event_id: capture_event_id.to_string(),
        pointer_event_id: format!("{}:pointer", proposal.trace_id),
        proposal_event_id: format!("{}:proposal", proposal.trace_id),
        decision_event_id: format!("{}:decision", proposal.trace_id),
        decision: decision.to_string(),
    }
}

fn entity_from_element(
    element: &UiSceneElement,
    request: &PointerContextRequest,
) -> StructuredTargetEntity {
    let label = if element.label.trim().is_empty() {
        element
            .text_masked
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&element.label)
    } else {
        &element.label
    };
    let sanitized_label = sanitize_title_with_level(label, PiiFilterLevel::Strict);
    let mut sources = BTreeSet::new();
    sources.insert("screenshot".to_string());
    if element.text_masked.is_some() || !element.label.trim().is_empty() {
        sources.insert("ocr".to_string());
    }
    if element.role.is_some() || element.intent.is_some() || element.state.is_some() {
        sources.insert("ax".to_string());
    }
    for source in &request.evidence_sources {
        sources.insert(source.to_ascii_lowercase());
    }

    let mut confidence = element.confidence;
    if request
        .selection_text
        .as_deref()
        .is_some_and(|selection| contains_normalized(&element.label, selection))
    {
        confidence += 0.25;
        sources.insert("selection".to_string());
    }
    if request
        .cursor_position
        .is_some_and(|(x, y)| element.bbox_abs.contains(x, y))
    {
        confidence += 0.2;
        sources.insert("cursor".to_string());
    }

    StructuredTargetEntity {
        entity_id: element.element_id.clone(),
        kind: element
            .role
            .clone()
            .unwrap_or_else(|| "visible-text".to_string()),
        label: sanitized_label.clone(),
        value: sanitized_label,
        bounds: element.bbox_abs.clone(),
        confidence: confidence.min(1.0),
        sources: sources.into_iter().collect(),
    }
}

fn contains_normalized(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.trim().to_ascii_lowercase();
    let needle = needle.trim().to_ascii_lowercase();
    !needle.is_empty() && haystack.contains(&needle)
}

fn is_deictic_instruction(instruction: &str) -> bool {
    let lower = instruction.to_ascii_lowercase();
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|token| matches!(token, "this" | "that" | "these" | "those"))
}

fn bounds_contains_bounds(container: &ElementBounds, child: &ElementBounds) -> bool {
    let child_right = child.x + child.width as i32;
    let child_bottom = child.y + child.height as i32;
    container.contains(child.x, child.y)
        && child_right <= container.x + container.width as i32
        && child_bottom <= container.y + container.height as i32
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use maekon_core::models::ui_scene::{NormalizedBounds, UI_SCENE_SCHEMA_VERSION};

    use super::*;

    fn element(
        id: &str,
        label: &str,
        role: Option<&str>,
        bounds: ElementBounds,
        confidence: f64,
    ) -> UiSceneElement {
        UiSceneElement {
            element_id: id.to_string(),
            bbox_abs: bounds,
            bbox_norm: NormalizedBounds::new(0.0, 0.0, 0.1, 0.1),
            label: label.to_string(),
            role: role.map(str::to_string),
            intent: None,
            state: Some("enabled".to_string()),
            confidence,
            text_masked: Some(sanitize_title_with_level(label, PiiFilterLevel::Standard)),
            parent_id: None,
        }
    }

    fn calculator_scene(elements: Vec<UiSceneElement>) -> UiScene {
        UiScene {
            schema_version: UI_SCENE_SCHEMA_VERSION.to_string(),
            scene_id: "scene-calculator".to_string(),
            app_name: Some("Calculator".to_string()),
            screen_id: Some("main".to_string()),
            captured_at: Utc::now(),
            screen_width: 640,
            screen_height: 480,
            elements,
        }
    }

    fn base_request(scene: UiScene) -> PointerContextRequest {
        PointerContextRequest {
            active_app: "Calculator".to_string(),
            active_window_title: Some("Calculator".to_string()),
            active_process: None,
            cursor_position: Some((110, 48)),
            selection_text: Some("46".to_string()),
            instruction: Some("Explain this".to_string()),
            scene,
            evidence_sources: vec!["screenshot".to_string()],
        }
    }

    #[test]
    fn resolves_cursor_or_selection_to_active_app_window_and_target_entity() {
        let scene = calculator_scene(vec![
            element(
                "display-result",
                "46",
                Some("calculator-display"),
                ElementBounds {
                    x: 80,
                    y: 24,
                    width: 180,
                    height: 52,
                },
                0.83,
            ),
            element(
                "button-4",
                "4",
                Some("button"),
                ElementBounds {
                    x: 80,
                    y: 120,
                    width: 48,
                    height: 48,
                },
                0.81,
            ),
        ]);

        let resolved = resolve_pointer_context(&base_request(scene));

        assert_eq!(resolved.active_app, "Calculator");
        assert_eq!(resolved.active_window_title.as_deref(), Some("Calculator"));
        let target = resolved.target.expect("target entity should resolve");
        assert_eq!(target.entity_id, "display-result");
        assert_eq!(target.kind, "calculator-display");
        assert!(target.sources.contains(&"cursor".to_string()));
        assert!(target.sources.contains(&"selection".to_string()));
    }

    #[test]
    fn fuses_ocr_ax_and_pixel_region_evidence_into_redacted_entities() {
        let secret = "token sk-test-1234567890abcdef";
        let scene = calculator_scene(vec![element(
            "secret-row",
            secret,
            Some("static-text"),
            ElementBounds {
                x: 10,
                y: 10,
                width: 300,
                height: 36,
            },
            0.92,
        )]);
        let mut request = base_request(scene);
        request.cursor_position = Some((40, 20));
        request.selection_text = Some("token".to_string());
        request.evidence_sources = vec!["ocr".to_string(), "ax".to_string(), "pixel".to_string()];

        let resolved = resolve_pointer_context(&request);
        let entity = resolved.target.expect("entity should resolve");

        assert!(entity.sources.contains(&"ocr".to_string()));
        assert!(entity.sources.contains(&"ax".to_string()));
        assert!(entity.sources.contains(&"pixel".to_string()));
        assert!(entity.sources.contains(&"screenshot".to_string()));
        assert!(!entity.value.contains("sk-test-1234567890abcdef"));
        assert!(entity.value.contains("[API_KEY]"));
    }

    #[test]
    fn this_that_instruction_resolves_or_asks_for_clarification_by_confidence() {
        let clear_scene = calculator_scene(vec![
            element(
                "display-result",
                "46",
                Some("calculator-display"),
                ElementBounds {
                    x: 80,
                    y: 24,
                    width: 180,
                    height: 52,
                },
                0.9,
            ),
            element(
                "button-4",
                "4",
                Some("button"),
                ElementBounds {
                    x: 80,
                    y: 120,
                    width: 48,
                    height: 48,
                },
                0.65,
            ),
        ]);
        let clear = resolve_pointer_context(&base_request(clear_scene));
        assert!(!clear.needs_clarification);
        assert_eq!(
            clear
                .target
                .as_ref()
                .map(|entity| entity.entity_id.as_str()),
            Some("display-result")
        );

        let ambiguous_scene = calculator_scene(vec![
            element(
                "candidate-a",
                "Total 46",
                Some("static-text"),
                ElementBounds {
                    x: 40,
                    y: 40,
                    width: 140,
                    height: 40,
                },
                0.82,
            ),
            element(
                "candidate-b",
                "Subtotal 46",
                Some("static-text"),
                ElementBounds {
                    x: 200,
                    y: 40,
                    width: 140,
                    height: 40,
                },
                0.8,
            ),
        ]);
        let mut ambiguous_request = base_request(ambiguous_scene);
        ambiguous_request.cursor_position = None;
        ambiguous_request.selection_text = None;
        let ambiguous = resolve_pointer_context(&ambiguous_request);
        assert!(ambiguous.needs_clarification);
        assert!(ambiguous.target.is_none());
    }

    #[test]
    fn pointer_action_proposal_is_highlighted_gated_and_audit_linked() {
        let scene = calculator_scene(vec![element(
            "display-result",
            "46",
            Some("calculator-display"),
            ElementBounds {
                x: 80,
                y: 24,
                width: 180,
                height: 52,
            },
            0.9,
        )]);
        let resolved = resolve_pointer_context(&base_request(scene));
        let proposal = build_pointer_action_proposal(&resolved, "explain_result")
            .expect("proposal should be built for resolved target");

        assert_eq!(proposal.target_entity_id, "display-result");
        assert!(proposal.policy_gate_visible);
        assert!(proposal.confirmation_required);
        assert!(proposal.action_not_auto_executed);
        assert_eq!(proposal.highlight_bounds.x, 80);
        let payload = proposal
            .allowed_action_payload
            .as_object()
            .expect("payload should be object");
        assert_eq!(payload.keys().count(), 3);
        assert!(payload.contains_key("target_entity_id"));
        assert!(payload.contains_key("action_kind"));
        assert!(payload.contains_key("requires_confirmation"));

        let trace = audit_trace_for_decision("capture-1", &proposal, "accepted");
        assert_eq!(trace.trace_id, proposal.trace_id);
        assert_eq!(trace.capture_event_id, "capture-1");
        assert_eq!(
            trace.pointer_event_id,
            format!("{}:pointer", proposal.trace_id)
        );
        assert_eq!(
            trace.proposal_event_id,
            format!("{}:proposal", proposal.trace_id)
        );
        assert_eq!(
            trace.decision_event_id,
            format!("{}:decision", proposal.trace_id)
        );
        assert_eq!(trace.decision, "accepted");
    }

    #[test]
    fn process_gui_anchor_keeps_suggestion_highlight_inside_active_window() {
        let scene = calculator_scene(vec![
            element(
                "display-result",
                "46",
                Some("calculator-display"),
                ElementBounds {
                    x: 228,
                    y: 174,
                    width: 184,
                    height: 52,
                },
                0.93,
            ),
            element(
                "button-equals",
                "=",
                Some("button"),
                ElementBounds {
                    x: 348,
                    y: 402,
                    width: 64,
                    height: 54,
                },
                0.81,
            ),
        ]);
        let mut request = base_request(scene);
        request.cursor_position = Some((320, 198));
        request.selection_text = Some("46".to_string());
        request.active_process = Some(ActiveProcessWindow {
            process_name: "Calculator".to_string(),
            bundle_id: Some("com.apple.calculator".to_string()),
            pid: Some(4242),
            screen_id: Some("main".to_string()),
            window_title: Some("Calculator".to_string()),
            window_bounds: ElementBounds {
                x: 200,
                y: 120,
                width: 260,
                height: 360,
            },
        });

        let resolved = resolve_pointer_context(&request);
        let anchor = build_process_gui_suggestion_anchor(&request, &resolved)
            .expect("target inside active process window should produce anchor");
        let proposal = build_pointer_action_proposal(&resolved, "explain_result")
            .expect("proposal should be built for resolved target");

        assert_eq!(anchor.process_name, "Calculator");
        assert_eq!(anchor.bundle_id.as_deref(), Some("com.apple.calculator"));
        assert_eq!(anchor.target_entity_id, "display-result");
        assert_eq!(anchor.target_label, "46");
        assert_eq!(anchor.target_center, (320, 200));
        assert!(anchor.target_inside_window);
        assert_eq!(anchor.highlight_bounds.x, proposal.highlight_bounds.x);
        assert_eq!(anchor.highlight_bounds.y, proposal.highlight_bounds.y);
        assert_eq!(
            anchor.highlight_bounds.width,
            proposal.highlight_bounds.width
        );
        assert_eq!(
            anchor.highlight_bounds.height,
            proposal.highlight_bounds.height
        );
        assert!(anchor.sources.contains(&"cursor".to_string()));
        assert!(anchor.sources.contains(&"selection".to_string()));
    }

    #[test]
    fn process_gui_anchor_rejects_target_outside_active_window() {
        let scene = calculator_scene(vec![element(
            "stale-notes-selection",
            "Quarterly roadmap note",
            Some("text-area"),
            ElementBounds {
                x: 520,
                y: 160,
                width: 260,
                height: 80,
            },
            0.96,
        )]);
        let mut request = base_request(scene);
        request.cursor_position = Some((560, 180));
        request.selection_text = Some("Quarterly roadmap".to_string());
        request.active_process = Some(ActiveProcessWindow {
            process_name: "Calculator".to_string(),
            bundle_id: Some("com.apple.calculator".to_string()),
            pid: Some(4242),
            screen_id: Some("main".to_string()),
            window_title: Some("Calculator".to_string()),
            window_bounds: ElementBounds {
                x: 200,
                y: 120,
                width: 260,
                height: 360,
            },
        });

        let resolved = resolve_pointer_context(&request);
        assert_eq!(
            resolved
                .target
                .as_ref()
                .map(|entity| entity.entity_id.as_str()),
            Some("stale-notes-selection")
        );
        assert!(
            build_process_gui_suggestion_anchor(&request, &resolved).is_none(),
            "out-of-window target must not produce a Calculator GUI guide anchor"
        );
    }
}
