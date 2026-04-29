use chrono::{DateTime, Utc};
use oneshim_core::models::embedding::{EmbeddingContentType, SearchResult};
use oneshim_core::quantization::QuantizedVector;

/// Convert a slice of f32 values to a little-endian byte vector.
pub fn f32_vec_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Convert a byte slice back to a `Vec<f32>` (little-endian).
pub fn bytes_to_f32_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Convert a slice of i8 values to a byte vector (for SQLite BLOB storage).
pub fn i8_vec_to_bytes(v: &[i8]) -> Vec<u8> {
    v.iter().map(|&b| b as u8).collect()
}

/// Convert a byte slice back to a `Vec<i8>`.
pub fn bytes_to_i8_vec(b: &[u8]) -> Vec<i8> {
    b.iter().map(|&b| b as i8).collect()
}

/// Compute cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

/// Row fetched from the embedding_vectors table for brute-force search.
pub struct VectorRow {
    pub segment_id: String,
    pub content_type: String,
    pub content_label: Option<String>,
    pub original_text: String,
    pub vector: Vec<f32>,
    pub timestamp: DateTime<Utc>,
}

/// Row fetched for INT8 brute-force search.
pub struct QuantizedVectorRow {
    pub segment_id: String,
    pub content_type: String,
    pub content_label: Option<String>,
    pub original_text: String,
    pub vector_int8: Vec<i8>,
    pub quant_scale: f32,
    pub quant_offset: f32,
    pub timestamp: DateTime<Utc>,
}

pub fn parse_content_type(s: &str) -> EmbeddingContentType {
    match s {
        "SEGMENT_SUMMARY" => EmbeddingContentType::SegmentSummary,
        _ => EmbeddingContentType::ContentActivity,
    }
}

/// Map a single SQLite row (with columns segment_id, content_type, content_label,
/// original_text, vector, timestamp at positions 0..5) to a `VectorRow`.
pub fn map_vector_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VectorRow> {
    let ts_str: String = row.get(5)?;
    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let blob: Vec<u8> = row.get(4)?;
    Ok(VectorRow {
        segment_id: row.get(0)?,
        content_type: row.get(1)?,
        content_label: row.get(2)?,
        original_text: row.get(3)?,
        vector: bytes_to_f32_vec(&blob),
        timestamp,
    })
}

/// Map a SQLite row (segment_id, content_type, content_label, original_text,
/// vector_int8, quant_scale, quant_offset, timestamp at positions 0..7)
/// to a QuantizedVectorRow.
pub fn map_quantized_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QuantizedVectorRow> {
    let ts_str: String = row.get(7)?;
    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let blob: Vec<u8> = row.get(4)?;
    Ok(QuantizedVectorRow {
        segment_id: row.get(0)?,
        content_type: row.get(1)?,
        content_label: row.get(2)?,
        original_text: row.get(3)?,
        vector_int8: bytes_to_i8_vec(&blob),
        quant_scale: row.get(5)?,
        quant_offset: row.get(6)?,
        timestamp,
    })
}

pub fn content_type_to_str(ct: &EmbeddingContentType) -> &'static str {
    match ct {
        EmbeddingContentType::SegmentSummary => "SEGMENT_SUMMARY",
        EmbeddingContentType::ContentActivity => "CONTENT_ACTIVITY",
    }
}

/// Execute brute-force search on rows, applying cosine similarity + time decay.
pub fn brute_force_search(
    rows: Vec<VectorRow>,
    query_vector: &[f32],
    limit: usize,
    time_decay_hours: f32,
) -> Vec<SearchResult> {
    let now = Utc::now();
    let mut scored: Vec<SearchResult> = rows
        .into_iter()
        .map(|row| {
            let similarity = cosine_similarity(query_vector, &row.vector);
            let age_hours = (now - row.timestamp).num_seconds().max(0) as f32 / 3600.0;
            let time_decay = if time_decay_hours > 0.0 {
                (-age_hours / time_decay_hours).exp()
            } else {
                1.0
            };
            let score = similarity * time_decay;
            SearchResult {
                segment_id: row.segment_id,
                content_type: parse_content_type(&row.content_type),
                content_label: row.content_label,
                score,
                similarity,
                time_decay,
                timestamp: row.timestamp,
                original_text: row.original_text,
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    scored
}

/// Execute brute-force search on INT8 quantized rows.
///
/// Uses `cosine_similarity_int8_unchecked` because all rows originate from
/// the same model and share the query's dimensionality.
pub fn brute_force_search_quantized(
    rows: Vec<QuantizedVectorRow>,
    query_vector: &QuantizedVector,
    limit: usize,
    time_decay_hours: f32,
) -> Vec<SearchResult> {
    let now = Utc::now();
    let mut scored: Vec<SearchResult> = rows
        .into_iter()
        .map(|row| {
            let row_qv = QuantizedVector {
                data: row.vector_int8,
                scale: row.quant_scale,
                offset: row.quant_offset,
            };
            let similarity =
                oneshim_core::quantization::ScalarQuantizer::cosine_similarity_int8_unchecked(
                    query_vector,
                    &row_qv,
                );
            let age_hours = (now - row.timestamp).num_seconds().max(0) as f32 / 3600.0;
            let time_decay = if time_decay_hours > 0.0 {
                (-age_hours / time_decay_hours).exp()
            } else {
                1.0
            };
            let score = similarity * time_decay;
            SearchResult {
                segment_id: row.segment_id,
                content_type: parse_content_type(&row.content_type),
                content_label: row.content_label,
                score,
                similarity,
                time_decay,
                timestamp: row.timestamp,
                original_text: row.original_text,
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    scored
}
