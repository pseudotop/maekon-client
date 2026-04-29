//! Shared attachment/content helpers for HTTP API sessions.

use std::path::Path;

use oneshim_api_contracts::provider_specs::ProviderRequestShape;
use oneshim_core::models::ai_session::{Attachment, ContentBlock, SessionMessage};

use super::anthropic::supports_anthropic_document;

pub(super) fn attachment_filename(path: Option<&str>) -> Option<String> {
    path.and_then(|value| {
        Path::new(value)
            .file_name()
            .and_then(|segment| segment.to_str())
            .map(ToOwned::to_owned)
    })
}

pub(super) fn native_content_block(
    shape: &ProviderRequestShape,
    attachment: &Attachment,
) -> Option<ContentBlock> {
    match attachment {
        Attachment::Image {
            mime,
            data: Some(data),
            ..
        } => Some(ContentBlock::Image {
            media_type: mime.clone(),
            data: data.clone(),
        }),
        Attachment::File {
            mime: Some(mime),
            data: Some(data),
            path,
        } if mime.starts_with("image/") => Some(ContentBlock::Image {
            media_type: mime.clone(),
            data: data.clone(),
        }),
        Attachment::File {
            mime: Some(mime),
            data: Some(data),
            path,
        } => {
            let filename = attachment_filename(Some(path));
            if matches!(
                shape,
                ProviderRequestShape::AnthropicMessages
                    | ProviderRequestShape::AnthropicVisionMessages
            ) && supports_anthropic_document(mime)
            {
                return Some(ContentBlock::File {
                    media_type: mime.clone(),
                    data: data.clone(),
                    filename,
                });
            }

            if matches!(
                shape,
                ProviderRequestShape::OpenAiResponses | ProviderRequestShape::GoogleGenerateContent
            ) {
                return Some(ContentBlock::File {
                    media_type: mime.clone(),
                    data: data.clone(),
                    filename,
                });
            }

            None
        }
        _ => None,
    }
}

pub(super) fn attachment_manifest_entry(attachment: &Attachment) -> serde_json::Value {
    match attachment {
        Attachment::Image { mime, path, data } => serde_json::json!({
            "kind": "image",
            "mime": mime,
            "path": path,
            "has_inline_data": data.as_ref().is_some_and(|value| !value.is_empty()),
        }),
        Attachment::File { path, mime, data } => serde_json::json!({
            "kind": "file",
            "path": path,
            "mime": mime,
            "has_inline_data": data.as_ref().is_some_and(|value| !value.is_empty()),
        }),
        Attachment::Directory { path } => serde_json::json!({
            "kind": "directory",
            "path": path,
        }),
        Attachment::Skill {
            skill_id,
            display_name,
        } => serde_json::json!({
            "kind": "skill",
            "skill_id": skill_id,
            "display_name": display_name,
        }),
        Attachment::AppReference {
            app_name,
            window_title,
        } => serde_json::json!({
            "kind": "app_reference",
            "app_name": app_name,
            "window_title": window_title,
        }),
    }
}

pub(super) fn render_message_content(
    message: &SessionMessage,
    shape: &ProviderRequestShape,
) -> String {
    let mut sections = vec![message.content.clone()];

    if let Some(context) = message.context.as_ref() {
        let has_context = context
            .regime
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || context
                .active_app
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
        if has_context {
            sections.push(format!(
                "Additional context JSON:\n{}",
                serde_json::to_string_pretty(context).unwrap_or_else(|_| "{}".to_string())
            ));
        }
    }

    let attachment_manifest: Vec<serde_json::Value> = message
        .attachments
        .iter()
        .filter(|attachment| native_content_block(shape, attachment).is_none())
        .map(attachment_manifest_entry)
        .collect();

    if !attachment_manifest.is_empty() {
        sections.push(format!(
            "Attachment manifest:\n{}",
            serde_json::to_string_pretty(&attachment_manifest).unwrap_or_else(|_| "[]".to_string())
        ));
    }

    sections.join("\n\n")
}

pub(super) fn empty_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}
