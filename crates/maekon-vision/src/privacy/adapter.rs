use maekon_core::config::PiiFilterLevel;

use super::detection::sanitize_title_with_level;

/// Adapter implementing [`maekon_core::ports::pii_sanitizer::PiiSanitizer`] by delegating to this module's
/// `sanitize_title_with_level` function.
pub struct VisionPiiSanitizer;

impl maekon_core::ports::pii_sanitizer::PiiSanitizer for VisionPiiSanitizer {
    fn sanitize_text(&self, text: &str, level: PiiFilterLevel) -> String {
        sanitize_title_with_level(text, level)
    }
}
