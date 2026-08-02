use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};
use image::{DynamicImage, Rgba, RgbaImage};
use maekon_core::config::PiiFilterLevel;
use maekon_core::config_manager::ConfigManager;
use maekon_core::models::event::{ContextEvent, Event};
use maekon_core::models::frame::FrameMetadata;
use maekon_storage::encryption::EncryptionKey;
use maekon_storage::frame_storage::FrameFileStorage;
use maekon_storage::sqlite::SqliteStorage;
use maekon_vision::encoder::{encode_webp, WebPQuality};
use maekon_vision::privacy::sanitize_title_with_level;

use crate::reauth::pin::{hash_pin, REAUTH_PIN_HASH_KEY};
use crate::storage_runtime::resolve_shared_master_key;

use super::types::SeedReport;
use super::{require_isolated_profile, MARKER_IN_PROGRESS, MARKER_KEY, MARKER_VERSION, PIN_ENV};

pub(crate) fn run_from_env() -> Result<SeedReport> {
    require_isolated_profile()?;
    let pin = std::env::var(PIN_ENV).context("MAEKON_TC_FIXTURE_PIN is required")?;

    let config_manager = ConfigManager::new().context("initialize isolated config")?;
    let config = config_manager
        .update_with(|config| {
            config.vision.capture_enabled = false;
            config.privacy.pii_filter_level = PiiFilterLevel::Standard;
            config.privacy.reauth.enabled = true;
            config.privacy.reauth.idle_timeout_secs = 3600;
            push_unique(&mut config.privacy.excluded_apps, "Notepad");
            push_unique(
                &mut config.privacy.excluded_title_patterns,
                "*MAEKON-QC-PRIVATE*",
            );
            Ok(())
        })
        .context("persist isolated QC config")?;

    let data_dir = ConfigManager::data_dir().context("resolve isolated data directory")?;
    std::fs::create_dir_all(&data_dir).context("create isolated data directory")?;
    let encryption_key =
        resolve_shared_master_key(&data_dir).context("resolve isolated profile encryption key")?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create QC fixture runtime")?;
    runtime.block_on(seed_fixture(
        &data_dir,
        encryption_key,
        config.storage.retention_days,
        config.storage.max_storage_mb,
        &pin,
    ))
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|candidate| candidate == value) {
        values.push(value.to_string());
    }
}

pub(super) async fn seed_fixture(
    data_dir: &Path,
    encryption_key: EncryptionKey,
    retention_days: u32,
    max_storage_mb: u64,
    pin: &str,
) -> Result<SeedReport> {
    let db_path = data_dir.join(maekon_storage::encryption::SQLCIPHER_DB_FILENAME);
    let storage = SqliteStorage::open(&db_path, retention_days, Some(&encryption_key))
        .context("open isolated encrypted QC database")?;

    match storage.get_meta(MARKER_KEY).as_deref() {
        Some(MARKER_VERSION) => {
            return Ok(SeedReport {
                data_dir: data_dir.display().to_string(),
                frames: 0,
                events: 0,
                already_seeded: true,
            });
        }
        Some(MARKER_IN_PROGRESS) => {
            bail!(
                "an earlier seed attempt did not finish; discard this isolated QC profile and retry"
            );
        }
        Some(other) => bail!("unsupported QC fixture marker version: {other}"),
        None => {}
    }

    let pin_hash = hash_pin(pin).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    storage
        .set_meta_checked(MARKER_KEY, MARKER_IN_PROGRESS)
        .context("mark QC fixture seed in progress")?;
    storage
        .set_meta_checked(REAUTH_PIN_HASH_KEY, &pin_hash)
        .context("store isolated fixture PIN verifier")?;

    let shared_key = Arc::new(encryption_key);
    let frame_storage = FrameFileStorage::with_encryption(
        data_dir.to_path_buf(),
        max_storage_mb,
        retention_days,
        Some(shared_key),
    )
    .await
    .context("initialize encrypted QC frame storage")?;

    let now = Utc::now();
    let seeds = fixture_frames();
    let mut frame_ids = Vec::with_capacity(seeds.len());
    let mut events = Vec::with_capacity(seeds.len());

    for (index, seed) in seeds.iter().enumerate() {
        let timestamp = now - Duration::minutes(seed.minutes_ago);
        let safe_title = sanitize_title_with_level(seed.window_title, PiiFilterLevel::Standard);
        let safe_ocr = sanitize_title_with_level(seed.ocr_text, PiiFilterLevel::Standard);
        ensure_sanitized(seed.window_title, &safe_title)?;
        ensure_sanitized(seed.ocr_text, &safe_ocr)?;

        let file_path = if seed.missing_image {
            Some("frames/qc-fixture/intentionally-missing.webp.enc".to_string())
        } else {
            let image = fixture_image(index, seed.color);
            let encoded = encode_webp(&DynamicImage::ImageRgba8(image), WebPQuality::Medium)
                .context("encode synthetic QC frame")?;
            Some(
                frame_storage
                    .save_frame(timestamp, &encoded)
                    .await
                    .context("save encrypted synthetic QC frame")?
                    .display()
                    .to_string(),
            )
        };

        let metadata = FrameMetadata {
            timestamp,
            trigger_type: "qc_fixture".to_string(),
            app_name: seed.app_name.to_string(),
            window_title: safe_title.clone(),
            resolution: (960, 540),
            importance: seed.importance,
            monitor_id: Some(0),
            app_bundle_id: Some(format!("qc.fixture.{}", seed.app_name.to_ascii_lowercase())),
        };
        let frame_id = storage
            .save_frame_metadata(&metadata, file_path.as_deref(), Some(&safe_ocr))
            .context("save synthetic QC frame metadata")?;
        frame_ids.push(frame_id);
        events.push(Event::Context(ContextEvent {
            app_name: seed.app_name.to_string(),
            window_title: safe_title,
            prev_app_name: index
                .checked_sub(1)
                .map(|previous| seeds[previous].app_name.to_string()),
            timestamp,
            input_activity_level: seed.importance,
        }));
    }

    let tag = storage
        .create_tag("qc-fixture", "#2dd4bf")
        .context("create QC fixture tag")?;
    for frame_id in frame_ids.iter().take(4) {
        storage
            .add_tag_to_frame(*frame_id, tag.id)
            .context("tag synthetic QC frame")?;
    }
    let delta_tag = storage
        .create_tag("delta", "#60a5fa")
        .context("create Delta search tag")?;
    for frame_id in frame_ids.iter().skip(2).take(3) {
        storage
            .add_tag_to_frame(*frame_id, delta_tag.id)
            .context("tag Delta synthetic frame")?;
    }

    let event_count = storage
        .save_events_batch(&events)
        .context("save synthetic QC replay events")?;
    storage
        .set_meta_checked(MARKER_KEY, MARKER_VERSION)
        .context("commit QC fixture marker")?;

    Ok(SeedReport {
        data_dir: data_dir.display().to_string(),
        frames: frame_ids.len(),
        events: event_count,
        already_seeded: false,
    })
}

pub(super) fn ensure_sanitized(raw: &str, sanitized: &str) -> Result<()> {
    for sensitive in ["qa.user+8324@example.com", "4111 1111 1111 1111"] {
        if raw.contains(sensitive) && sanitized.contains(sensitive) {
            bail!("privacy sanitizer left a synthetic sensitive decoy unchanged")
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct FixtureFrame {
    minutes_ago: i64,
    app_name: &'static str,
    window_title: &'static str,
    pub(super) ocr_text: &'static str,
    importance: f32,
    color: [u8; 3],
    missing_image: bool,
}

pub(super) fn fixture_frames() -> [FixtureFrame; 8] {
    [
        FixtureFrame {
            minutes_ago: 4,
            app_name: "Maekon QC Notes",
            window_title: "Project Delta launch checklist",
            ocr_text: "Project Delta review completed. Open the replay timeline for provenance.",
            importance: 0.92,
            color: [13, 148, 136],
            missing_image: false,
        },
        FixtureFrame {
            minutes_ago: 8,
            app_name: "Maekon QC Browser",
            window_title: "Delta research - local fixture",
            ocr_text: "Keyword target: glacier-orbit. This content is synthetic and local only.",
            importance: 0.84,
            color: [37, 99, 235],
            missing_image: false,
        },
        FixtureFrame {
            minutes_ago: 13,
            app_name: "Maekon QC Mail",
            window_title: "Project Delta follow-up for qa.user+8324@example.com",
            ocr_text: "Synthetic contact qa.user+8324@example.com and card 4111 1111 1111 1111 must be redacted.",
            importance: 0.78,
            color: [124, 58, 237],
            missing_image: false,
        },
        FixtureFrame {
            minutes_ago: 19,
            app_name: "Maekon QC Editor",
            window_title: "Delta annotation workspace",
            ocr_text: "Annotation fixture: add, review, then remove a temporary note.",
            importance: 0.73,
            color: [217, 119, 6],
            missing_image: false,
        },
        FixtureFrame {
            minutes_ago: 26,
            app_name: "Maekon QC Terminal",
            window_title: "glacier-orbit keyword trace",
            ocr_text: "glacier-orbit exact keyword result with replay event context.",
            importance: 0.66,
            color: [22, 163, 74],
            missing_image: false,
        },
        FixtureFrame {
            minutes_ago: 34,
            app_name: "Maekon QC Browser",
            window_title: "Ambiguous Delta result A",
            ocr_text: "Delta could refer to the launch checklist or the research workspace.",
            importance: 0.61,
            color: [225, 29, 72],
            missing_image: false,
        },
        FixtureFrame {
            minutes_ago: 43,
            app_name: "Maekon QC Notes",
            window_title: "Ambiguous Delta result B",
            ocr_text: "Another Delta context exists; ask for app or time clarification.",
            importance: 0.58,
            color: [8, 145, 178],
            missing_image: false,
        },
        FixtureFrame {
            minutes_ago: 51,
            app_name: "Maekon QC Archive",
            window_title: "Intentionally missing local frame",
            ocr_text: "Metadata remains available when the encrypted frame file is unavailable.",
            importance: 0.55,
            color: [100, 116, 139],
            missing_image: true,
        },
    ]
}

fn fixture_image(index: usize, color: [u8; 3]) -> RgbaImage {
    let mut image = RgbaImage::from_pixel(960, 540, Rgba([color[0], color[1], color[2], 255]));
    let accent = Rgba([
        color[0].saturating_add(32),
        color[1].saturating_add(32),
        color[2].saturating_add(32),
        255,
    ]);
    let stripe_start = 40 + (index as u32 * 23 % 240);
    for y in stripe_start..(stripe_start + 80) {
        for x in 64..896 {
            image.put_pixel(x, y, accent);
        }
    }
    image
}
