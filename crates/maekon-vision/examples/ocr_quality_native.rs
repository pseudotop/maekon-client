//! #12057: one synthetic PNG through the existing native OCR port.
//!
//! The private collector verifies the frozen corpus before sending bytes.
//! This example never captures a screen, selects a remote provider, downloads
//! a model, or changes the production privacy/sanitization path.

use std::io::{self, Read, Write};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maekon_core::ports::ocr_provider::{OcrProvider, OcrResult};
#[cfg(feature = "native-vision")]
use maekon_vision::native_ocr::{create_native_ocr, default_ocr_languages};
use serde::{Deserialize, Serialize};

const MAX_INPUT_BYTES: u64 = 4 * 1024 * 1024;
const PROTOCOL: &str = "maekon.synthetic-native-ocr.v1";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    sample_id: String,
    asset_sha256: String,
    synthetic_only: bool,
    image_base64: String,
}

#[derive(Serialize)]
struct Event<'a> {
    protocol: &'static str,
    event: &'static str,
    sample_id: &'a str,
    asset_sha256: &'a str,
    source_sha: Option<&'static str>,
    provider: Option<&'a str>,
    os: &'static str,
    architecture: &'static str,
    languages: Vec<String>,
    features: Vec<&'static str>,
    results: Option<&'a [OcrResult]>,
    reason: Option<&'static str>,
}

fn emit(event: &Event<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, event)?;
    writeln!(&mut output)?;
    output.flush()?;
    Ok(())
}

fn event<'a>(input: &'a Input, kind: &'static str, provider: Option<&'a str>) -> Event<'a> {
    Event {
        protocol: PROTOCOL,
        event: kind,
        sample_id: &input.sample_id,
        asset_sha256: &input.asset_sha256,
        source_sha: option_env!("MAEKON_OCR_SOURCE_SHA"),
        provider,
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        #[cfg(feature = "native-vision")]
        languages: default_ocr_languages(),
        #[cfg(not(feature = "native-vision"))]
        languages: Vec::new(),
        features: [
            (cfg!(feature = "native-vision"), "native-vision"),
            (cfg!(feature = "linux-atspi"), "linux-atspi"),
        ]
        .into_iter()
        .filter_map(|(enabled, name)| enabled.then_some(name))
        .collect(),
        results: None,
        reason: None,
    }
}

fn validate(input: &Input) -> Result<Vec<u8>, &'static str> {
    if !input.synthetic_only
        || input.sample_id.is_empty()
        || input.sample_id.len() > 128
        || !input
            .sample_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        || input.asset_sha256.len() != 64
        || !input.asset_sha256.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err("synthetic_input_contract");
    }
    let bytes = STANDARD
        .decode(&input.image_base64)
        .map_err(|_| "invalid_base64")?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("png_required");
    }
    Ok(bytes)
}

async fn extract_full(
    provider: &dyn OcrProvider,
    bytes: &[u8],
) -> Result<Vec<OcrResult>, &'static str> {
    if provider.is_external() || !provider.egress_endpoint_urls().is_empty() {
        return Err("external_provider_forbidden");
    }
    provider
        .extract_elements(bytes, "png")
        .await
        .map_err(|_| "native_ocr_error")
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw = Vec::new();
    io::stdin()
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut raw)?;
    if raw.len() as u64 > MAX_INPUT_BYTES {
        return Err("input_too_large".into());
    }
    let input: Input = serde_json::from_slice(&raw)?;
    let bytes = validate(&input)?;
    #[cfg(feature = "native-vision")]
    let candidate = create_native_ocr();
    #[cfg(not(feature = "native-vision"))]
    let candidate: Option<std::sync::Arc<dyn OcrProvider>> = None;
    let Some(provider) = candidate else {
        let mut unavailable = event(&input, "unavailable", None);
        unavailable.reason = Some("native_ocr_platform_unavailable");
        emit(&unavailable)?;
        return Ok(());
    };
    if provider.is_external() || !provider.egress_endpoint_urls().is_empty() {
        return Err("external_provider_forbidden".into());
    }
    // This marks an attempted port call. Only a completed result demonstrates
    // successful native execution; a timed-out attempt remains uncertain.
    emit(&event(
        &input,
        "call_attempted",
        Some(provider.provider_name()),
    ))?;
    match extract_full(provider.as_ref(), &bytes).await {
        Ok(results) => {
            let mut completed = event(&input, "completed", Some(provider.provider_name()));
            completed.results = Some(&results);
            emit(&completed)?;
        }
        Err(reason) => {
            let mut failed = event(&input, "error", Some(provider.provider_name()));
            failed.reason = Some(reason);
            emit(&failed)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::ports::ocr_provider::FakeOcrProvider;

    fn results() -> Vec<OcrResult> {
        (0..12)
            .map(|i| OcrResult {
                text: format!("field{i}={i}"),
                x: 0,
                y: i * 20,
                width: 100,
                height: 14,
                confidence: 0.9,
            })
            .collect()
    }

    #[tokio::test]
    async fn preserves_every_result_beyond_the_benchmark_summary_limit(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let provider = FakeOcrProvider::new(results());
        let full = extract_full(&provider, b"synthetic").await?;
        assert_eq!(full.len(), 12);
        assert_eq!(full[11].text, "field11=11");
        let encoded = serde_json::to_string(&full)?;
        let decoded: Vec<OcrResult> = serde_json::from_str(&encoded)?;
        assert_eq!(decoded.len(), 12);
        Ok(())
    }

    #[tokio::test]
    async fn external_provider_is_rejected_before_output_can_be_consumed() {
        let provider = FakeOcrProvider::new(results()).external();
        assert_eq!(
            extract_full(&provider, b"synthetic").await.err(),
            Some("external_provider_forbidden")
        );
    }
}
