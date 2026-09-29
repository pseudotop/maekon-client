use super::*;
use maekon_core::models::candidate_decision::DecisionOption;

fn text(goal: &str, labels: &[&str]) -> DecisionText {
    DecisionText {
        goal: goal.into(),
        candidates: labels
            .iter()
            .enumerate()
            .map(|(index, label)| DecisionOption {
                id: format!("c{index}"),
                text: (*label).into(),
                role: None,
                intent: None,
                state: None,
            })
            .collect(),
    }
}

#[test]
fn local_candidate_rules_select_the_unique_label_without_preferring_the_first() {
    for (goal, labels, id) in [
        ("Save", vec!["Cancel", "Save"], "c1"),
        (" save ", vec!["SAVE", "Cancel"], "c0"),
        ("저장", vec!["취소", " 저장 "], "c1"),
    ] {
        assert_eq!(
            local_exact_label_choice(&text(goal, &labels), true),
            (LocalAssessmentChoice::Selected(id.into()), 1)
        );
    }
}

#[test]
fn local_candidate_rules_delegate_ambiguous_absent_and_negated_goals() {
    for (goal, labels, count) in [
        ("Save", vec!["Save", " save "], 2),
        ("Save", vec!["Cancel", "Close"], 0),
        ("Do not save", vec!["Save"], 0),
    ] {
        assert_eq!(
            local_exact_label_choice(&text(goal, &labels), true),
            (LocalAssessmentChoice::Delegate, count)
        );
    }
}

#[test]
fn local_candidate_rules_do_not_turn_empty_labels_into_matches() {
    for labels in [vec![""], vec![" ", ""]] {
        assert_eq!(
            local_exact_label_choice(&text(" ", &labels), true),
            (LocalAssessmentChoice::Delegate, 0)
        );
    }
}

#[test]
fn local_candidate_rules_reject_redaction_matches_with_a_positive_control() {
    let sanitized = text("[EMAIL]", &["[EMAIL]"]);
    assert_eq!(
        local_exact_label_choice(&sanitized, true),
        (LocalAssessmentChoice::Selected("c0".into()), 1)
    );
    assert_eq!(
        local_exact_label_choice(&sanitized, false),
        (LocalAssessmentChoice::Delegate, 0)
    );
}
