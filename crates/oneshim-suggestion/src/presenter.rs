use oneshim_core::models::suggestion::{Priority, Suggestion, SuggestionType};

#[derive(Debug, Clone)]
pub struct SuggestionView {
    pub id: String,
    pub title: String,
    pub body: String,
    pub priority_label: String,
    pub priority_color: String,
    pub type_icon: String,
    pub confidence_text: String,
    pub is_actionable: bool,
    pub time_text: String,
}

pub fn present(suggestion: &Suggestion) -> SuggestionView {
    SuggestionView {
        id: suggestion.suggestion_id.clone(),
        title: type_to_title(&suggestion.suggestion_type),
        body: suggestion.content.clone(),
        priority_label: priority_to_label(&suggestion.priority),
        priority_color: priority_to_color(&suggestion.priority),
        type_icon: type_to_icon(&suggestion.suggestion_type),
        confidence_text: format!("{:.0}%", suggestion.confidence_score * 100.0),
        is_actionable: suggestion.is_actionable,
        time_text: format_relative_time(suggestion.created_at),
    }
}

pub fn present_all(suggestions: &[Suggestion]) -> Vec<SuggestionView> {
    suggestions.iter().map(present).collect()
}

pub fn type_to_title(st: &SuggestionType) -> String {
    match st {
        SuggestionType::WorkGuidance => "Work Guidance".to_string(),
        SuggestionType::EmailDraft => "Email Draft".to_string(),
        SuggestionType::ProductivityTip => "Productivity Tip".to_string(),
        SuggestionType::WorkflowOptimization => "Workflow Optimization".to_string(),
        SuggestionType::ContextBased => "Context-Based Suggestion".to_string(),
    }
}

fn type_to_icon(st: &SuggestionType) -> String {
    match st {
        SuggestionType::WorkGuidance => "compass".to_string(),
        SuggestionType::EmailDraft => "mail".to_string(),
        SuggestionType::ProductivityTip => "zap".to_string(),
        SuggestionType::WorkflowOptimization => "git-branch".to_string(),
        SuggestionType::ContextBased => "brain".to_string(),
    }
}

fn priority_to_label(p: &Priority) -> String {
    match p {
        Priority::Critical => "Critical".to_string(),
        Priority::High => "High".to_string(),
        Priority::Medium => "Medium".to_string(),
        Priority::Low => "Low".to_string(),
    }
}

fn priority_to_color(p: &Priority) -> String {
    match p {
        Priority::Critical => "#EF4444".to_string(), // red-500
        Priority::High => "#F97316".to_string(),     // orange-500
        Priority::Medium => "#3B82F6".to_string(),   // blue-500
        Priority::Low => "#6B7280".to_string(),      // gray-500
    }
}

fn format_relative_time(dt: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let diff = now - dt;

    if diff.num_seconds() < 60 {
        "Just now".to_string()
    } else if diff.num_minutes() < 60 {
        format!("{}m ago", diff.num_minutes())
    } else if diff.num_hours() < 24 {
        format!("{}h ago", diff.num_hours())
    } else {
        format!("{}d ago", diff.num_days())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_suggestion() -> Suggestion {
        Suggestion {
            suggestion_id: "sug_001".to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: "test suggestion".to_string(),
            priority: Priority::High,
            confidence_score: 0.95,
            relevance_score: 0.88,
            is_actionable: true,
            created_at: Utc::now(),
            expires_at: None,
            source: Default::default(),
            reasoning: None,
        }
    }

    #[test]
    fn present_suggestion() {
        let view = present(&make_suggestion());
        assert_eq!(view.id, "sug_001");
        assert_eq!(view.title, "Work Guidance");
        assert_eq!(view.priority_label, "High");
        assert_eq!(view.priority_color, "#F97316");
        assert_eq!(view.confidence_text, "95%");
        assert!(view.is_actionable);
    }

    #[test]
    fn present_all_suggestions() {
        let suggestions = vec![make_suggestion(), make_suggestion()];
        let views = present_all(&suggestions);
        assert_eq!(views.len(), 2);
    }

    #[test]
    fn priority_colors() {
        assert_eq!(priority_to_color(&Priority::Critical), "#EF4444");
        assert_eq!(priority_to_color(&Priority::High), "#F97316");
        assert_eq!(priority_to_color(&Priority::Medium), "#3B82F6");
        assert_eq!(priority_to_color(&Priority::Low), "#6B7280");
    }

    #[test]
    fn type_titles() {
        assert_eq!(type_to_title(&SuggestionType::EmailDraft), "Email Draft");
        assert_eq!(
            type_to_title(&SuggestionType::ProductivityTip),
            "Productivity Tip"
        );
    }
}
