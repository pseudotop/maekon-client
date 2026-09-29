//! Pure advisory matching for already validated and sanitized candidate text.

use maekon_core::models::candidate_assessment::LocalAssessmentChoice;
use maekon_core::models::candidate_decision::DecisionText;

/// Return a unique trimmed, lowercased label match and the observed match count.
/// The caller validates text bounds and establishes whether redaction changed
/// the goal or labels. This function grants no data, inference or execution authority.
pub fn local_exact_label_choice(
    text: &DecisionText,
    unchanged: bool,
) -> (LocalAssessmentChoice, usize) {
    if !unchanged {
        return (LocalAssessmentChoice::Delegate, 0);
    }
    let goal = text.goal.trim().to_lowercase();
    let matches: Vec<_> = text
        .candidates
        .iter()
        .filter(|candidate| {
            !candidate.text.trim().is_empty() && candidate.text.trim().to_lowercase() == goal
        })
        .collect();
    if let [candidate] = matches.as_slice() {
        (LocalAssessmentChoice::Selected(candidate.id.clone()), 1)
    } else {
        (LocalAssessmentChoice::Delegate, matches.len())
    }
}

#[cfg(test)]
#[path = "local_candidate_rules_tests.rs"]
mod tests;
