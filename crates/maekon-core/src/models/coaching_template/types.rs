//! `CoachingTemplate` struct — coaching message template with variable placeholders.

use crate::config::CoachingTone;
use crate::models::coaching::CoachingProfile;

/// A coaching message template with variable placeholders.
///
/// Placeholders use `{variable_name}` syntax, resolved at runtime by the
/// template selection engine in `maekon-analysis`.
#[derive(Debug, Clone)]
pub struct CoachingTemplate {
    pub profile: CoachingProfile,
    pub trigger_type: &'static str,
    pub tone: CoachingTone,
    pub locale: &'static str,
    pub text: &'static str,
}
