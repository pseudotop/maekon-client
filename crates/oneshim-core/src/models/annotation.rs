use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// User-created annotation attached to a captured frame.
///
/// Supports highlights, memos, and arrows overlaid on frame screenshots
/// for bookmarking and note-taking during post-session review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameAnnotation {
    pub annotation_id: String,
    /// Foreign key referencing `frames` table INTEGER PRIMARY KEY.
    pub frame_id: i64,
    pub annotation_type: AnnotationType,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Option<String>,
    pub text: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Kind of annotation drawn on a frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AnnotationType {
    Highlight,
    Memo,
    Arrow,
}

impl AnnotationType {
    /// Serialize to a stable string representation for SQLite TEXT column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Highlight => "Highlight",
            Self::Memo => "Memo",
            Self::Arrow => "Arrow",
        }
    }

    /// Deserialize from a SQLite TEXT column value.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "Highlight" => Self::Highlight,
            "Memo" => Self::Memo,
            "Arrow" => Self::Arrow,
            _ => Self::Highlight, // fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_type_roundtrip() {
        for ty in [
            AnnotationType::Highlight,
            AnnotationType::Memo,
            AnnotationType::Arrow,
        ] {
            assert_eq!(AnnotationType::from_str_lossy(ty.as_str()), ty);
        }
    }

    #[test]
    fn annotation_type_fallback() {
        assert_eq!(
            AnnotationType::from_str_lossy("Unknown"),
            AnnotationType::Highlight
        );
    }

    #[test]
    fn frame_annotation_serde_roundtrip() {
        let annotation = FrameAnnotation {
            annotation_id: "ann-001".to_string(),
            frame_id: 42,
            annotation_type: AnnotationType::Memo,
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
            color: Some("#ff0000".to_string()),
            text: Some("Important note".to_string()),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&annotation).unwrap();
        let deserialized: FrameAnnotation = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.annotation_id, "ann-001");
        assert_eq!(deserialized.annotation_type, AnnotationType::Memo);
        assert_eq!(deserialized.frame_id, 42);
    }
}
