use serde::Deserialize;
use serde_json::Value;

use maekon_core::error::CoreError;
use maekon_core::ports::ocr_provider::OcrResult;

use super::RemoteOcrProvider;

impl RemoteOcrProvider {
    pub(super) fn parse_claude_vision_response(body: &str) -> Result<Vec<OcrResult>, CoreError> {
        let response: serde_json::Value =
            serde_json::from_str(body).map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
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
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
            message: format!("Failed to parse OpenAI response: {e}"),
        })?;

        let text = Self::extract_openai_text(&response).ok_or_else(|| CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
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
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("Failed to parse generic response: {}", e),
            })?;

        response.results.ok_or_else(|| CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
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
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
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

    // Saturate raw i64 coordinates into the i32 range so an out-of-range
    // adversarial value clamps to the boundary instead of silently
    // truncating/wrapping via a narrowing `as i32` cast.
    let clamp_coord = |raw: i64| raw.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let points: Vec<(i32, i32)> = vertices
        .iter()
        .map(|vertex| {
            let x = clamp_coord(vertex.get("x").and_then(|v| v.as_i64()).unwrap_or(0));
            let y = clamp_coord(vertex.get("y").and_then(|v| v.as_i64()).unwrap_or(0));
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

    // Compute extents with widening (i64) arithmetic so adversarial
    // coordinates (e.g. min = i32::MIN, max = i32::MAX) cannot overflow the
    // i32 subtraction. Clamp the widened difference into the u32 range before
    // narrowing, making the parse total (no panic, no wraparound).
    let width = (i64::from(max_x) - i64::from(min_x)).clamp(0, u32::MAX as i64) as u32;
    let height = (i64::from(max_y) - i64::from(min_y)).clamp(0, u32::MAX as i64) as u32;

    (min_x, min_y, width, height)
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

#[cfg(test)]
mod bounding_vertices_tests {
    use super::*;
    use serde_json::json;

    fn vertices(points: &[(i64, i64)]) -> Vec<serde_json::Value> {
        points
            .iter()
            .map(|(x, y)| json!({ "x": x, "y": y }))
            .collect()
    }

    #[test]
    fn parses_normal_bounding_box() {
        let verts = vertices(&[(10, 20), (110, 20), (110, 60), (10, 60)]);
        let (x, y, width, height) = parse_bounding_vertices(Some(&verts));
        assert_eq!((x, y, width, height), (10, 20, 100, 40));
    }

    /// Regression guard (ocr-vertex-overflow): adversarial Google Vision
    /// coordinates spanning the full i32 range must not panic via an
    /// overflowing i32 subtraction. The extent is computed with widening
    /// i64 arithmetic and clamped into the u32 range.
    #[test]
    fn extreme_coordinates_do_not_panic_and_clamp_extent() {
        let verts = vertices(&[
            (i64::from(i32::MIN), i64::from(i32::MIN)),
            (i64::from(i32::MAX), i64::from(i32::MAX)),
        ]);
        // Must not panic; full i32 span saturates the u32 extent.
        let (x, y, width, height) = parse_bounding_vertices(Some(&verts));
        assert_eq!(x, i32::MIN);
        assert_eq!(y, i32::MIN);
        // (i32::MAX - i32::MIN) == u32::MAX exactly, so the extent is u32::MAX.
        assert_eq!(width, u32::MAX);
        assert_eq!(height, u32::MAX);
    }

    /// Out-of-i32-range JSON numbers are saturated into the i32 range when
    /// narrowed, instead of wrapping. The resulting extent stays clamped.
    #[test]
    fn out_of_range_json_coords_saturate_without_wrapping() {
        let verts = vertices(&[(i64::MIN, i64::MIN), (i64::MAX, i64::MAX)]);
        let (x, y, width, height) = parse_bounding_vertices(Some(&verts));
        assert_eq!(x, i32::MIN);
        assert_eq!(y, i32::MIN);
        assert_eq!(width, u32::MAX);
        assert_eq!(height, u32::MAX);
    }

    /// Inverted vertex order (max appears before min) must yield a
    /// non-negative, clamped extent rather than a negative-then-wrapped one.
    #[test]
    fn inverted_extent_clamps_to_zero() {
        let verts = vertices(&[(100, 100), (10, 10)]);
        let (x, y, width, height) = parse_bounding_vertices(Some(&verts));
        assert_eq!((x, y, width, height), (10, 10, 90, 90));
    }

    #[test]
    fn missing_vertices_returns_zero_box() {
        assert_eq!(parse_bounding_vertices(None), (0, 0, 0, 0));
        let empty: Vec<serde_json::Value> = Vec::new();
        assert_eq!(parse_bounding_vertices(Some(&empty)), (0, 0, 0, 0));
    }
}
