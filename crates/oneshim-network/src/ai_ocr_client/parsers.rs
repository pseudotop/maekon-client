use serde::Deserialize;
use serde_json::Value;

use oneshim_core::error::CoreError;
use oneshim_core::ports::ocr_provider::OcrResult;

use super::RemoteOcrProvider;

impl RemoteOcrProvider {
    pub(super) fn parse_claude_vision_response(body: &str) -> Result<Vec<OcrResult>, CoreError> {
        let response: serde_json::Value =
            serde_json::from_str(body).map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("Failed to parse response JSON: {}", e),
            })?;

        let mut results = Vec::new();

        if let Some(content) = response.get("content").and_then(|c| c.as_array()) {
            for block in content {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    for (i, line) in text.lines().enumerate() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            results.push(OcrResult {
                                text: trimmed.to_string(),
                                x: 0,
                                y: (i as i32) * 20, // temporary line height
                                width: (trimmed.len() as u32) * 8, // temporary char width
                                height: 20,
                                confidence: 0.8, // API default confidence
                            });
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    pub(super) fn parse_openai_vision_response(body: &str) -> Result<Vec<OcrResult>, CoreError> {
        if let Ok(results) = Self::parse_generic_response(body) {
            return Ok(results);
        }

        let response: Value = serde_json::from_str(body).map_err(|e| CoreError::OcrError {
            code: oneshim_core::error_codes::ProviderCode::OcrFailed,
            message: format!("Failed to parse OpenAI response: {e}"),
        })?;

        let text = Self::extract_openai_text(&response).ok_or_else(|| CoreError::OcrError {
            code: oneshim_core::error_codes::ProviderCode::OcrFailed,
            message: "No text content found in OpenAI OCR response".to_string(),
        })?;

        if let Some(json_fragment) = extract_json_fragment(&text) {
            if let Ok(results) = Self::parse_generic_response(&json_fragment) {
                return Ok(results);
            }
        }

        Ok(parse_text_lines_to_results(&text))
    }

    pub(super) fn parse_generic_response(body: &str) -> Result<Vec<OcrResult>, CoreError> {
        #[derive(Deserialize)]
        struct GenericResponse {
            results: Option<Vec<OcrResult>>,
        }

        let response: GenericResponse =
            serde_json::from_str(body).map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("Failed to parse generic response: {}", e),
            })?;

        response.results.ok_or_else(|| CoreError::OcrError {
            code: oneshim_core::error_codes::ProviderCode::OcrFailed,
            message: "Generic OCR response missing `results` field".to_string(),
        })
    }

    fn extract_openai_text(response: &Value) -> Option<String> {
        if let Some(content) = response
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|msg| msg.get("content"))
        {
            if let Some(text) = value_to_text(content) {
                return Some(text);
            }
        }

        let mut chunks = Vec::new();
        if let Some(outputs) = response.get("output").and_then(|o| o.as_array()) {
            for output in outputs {
                if let Some(parts) = output.get("content").and_then(|c| c.as_array()) {
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                chunks.push(trimmed.to_string());
                            }
                        }
                    }
                }
            }
        }

        if chunks.is_empty() {
            None
        } else {
            Some(chunks.join("\n"))
        }
    }

    pub(super) fn parse_google_vision_response(body: &str) -> Result<Vec<OcrResult>, CoreError> {
        let response: serde_json::Value =
            serde_json::from_str(body).map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("Failed to parse Google Vision response: {}", e),
            })?;

        let mut results = Vec::new();
        let annotations = response
            .get("responses")
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.first())
            .and_then(|entry| entry.get("textAnnotations"))
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();

        for annotation in annotations {
            let text = annotation
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if text.is_empty() {
                continue;
            }

            let vertices = annotation
                .get("boundingPoly")
                .and_then(|poly| poly.get("vertices"))
                .and_then(|v| v.as_array());
            let (x, y, width, height) = parse_bounding_vertices(vertices);

            results.push(OcrResult {
                text: text.to_string(),
                x,
                y,
                width,
                height,
                confidence: 0.8,
            });
        }

        Ok(results)
    }
}

pub(super) fn parse_bounding_vertices(
    vertices: Option<&Vec<serde_json::Value>>,
) -> (i32, i32, u32, u32) {
    let Some(vertices) = vertices else {
        return (0, 0, 0, 0);
    };

    let points: Vec<(i32, i32)> = vertices
        .iter()
        .map(|vertex| {
            let x = vertex.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let y = vertex.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            (x, y)
        })
        .collect();

    if points.is_empty() {
        return (0, 0, 0, 0);
    }

    let min_x = points.iter().map(|(x, _)| *x).min().unwrap_or(0);
    let max_x = points.iter().map(|(x, _)| *x).max().unwrap_or(0);
    let min_y = points.iter().map(|(_, y)| *y).min().unwrap_or(0);
    let max_y = points.iter().map(|(_, y)| *y).max().unwrap_or(0);

    (
        min_x,
        min_y,
        (max_x - min_x).max(0) as u32,
        (max_y - min_y).max(0) as u32,
    )
}

fn value_to_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(items) = value.as_array() {
        let mut parts = Vec::new();
        for item in items {
            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }
        }
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }

    None
}

fn extract_json_fragment(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    Some(text[start..=end].to_string())
}

pub(super) fn parse_text_lines_to_results(text: &str) -> Vec<OcrResult> {
    text.lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            Some(OcrResult {
                text: trimmed.to_string(),
                x: 0,
                y: (idx as i32) * 20,
                width: (trimmed.len() as u32) * 8,
                height: 20,
                confidence: 0.8,
            })
        })
        .collect()
}
