//! Debug pointer-capture evidence probe.

use super::{DebugPointerCaptureCliCommand, DebugPointerCaptureRuntimeCliCommand};
use image::GenericImageView;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const SIGNAL_SCAN_STRIDE_PX: u32 = 2;
const EDGE_SIGNAL_MIN_PIXELS: u64 = 64;
const POINTER_SIGNAL_MIN_PIXELS: u64 = 8;
const POINTER_SIGNAL_MIN_RADIUS_PX: i64 = 12;
const POINTER_SIGNAL_MAX_RADIUS_PX: i64 = 38;
const OVERLAY_PROBE_WEBVIEW_WARMUP_MS: u64 = 1_200;
const OVERLAY_PROBE_POINTER_EMIT_ATTEMPTS: u32 = 5;
const OVERLAY_PROBE_POINTER_EMIT_INTERVAL_MS: u64 = 120;
const OVERLAY_PROBE_PRE_CAPTURE_SETTLE_MS: u64 = 300;

#[derive(Default)]
struct SignalStats {
    total: u64,
    edge: u64,
    interior: u64,
    pointer: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalPointerPosition {
    x: u32,
    y: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PointerPositionSnapshot {
    global_x: i32,
    global_y: i32,
    local: Option<LocalPointerPosition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverlayProbeTimingContract {
    warmup_before_first_emit: Duration,
    emit_attempts: u32,
    emit_interval: Duration,
    settle_after_last_emit: Duration,
}

fn overlay_probe_timing_contract() -> OverlayProbeTimingContract {
    OverlayProbeTimingContract {
        warmup_before_first_emit: Duration::from_millis(OVERLAY_PROBE_WEBVIEW_WARMUP_MS),
        emit_attempts: OVERLAY_PROBE_POINTER_EMIT_ATTEMPTS,
        emit_interval: Duration::from_millis(OVERLAY_PROBE_POINTER_EMIT_INTERVAL_MS),
        settle_after_last_emit: Duration::from_millis(OVERLAY_PROBE_PRE_CAPTURE_SETTLE_MS),
    }
}

pub(crate) fn run_debug_pointer_capture_cli_command(command: DebugPointerCaptureCliCommand) -> i32 {
    let output_dir = pointer_capture_output_dir();
    let payload = match command {
        DebugPointerCaptureCliCommand::Probe {
            frames,
            interval_ms,
        } => run_pointer_capture_probe(&output_dir, frames, interval_ms),
    };

    emit_pointer_capture_payload(&output_dir, &payload)
}

pub(crate) fn run_debug_pointer_capture_runtime_cli_command(
    app_handle: &AppHandle,
    command: DebugPointerCaptureRuntimeCliCommand,
) -> i32 {
    let output_dir = pointer_capture_output_dir();
    let payload = match command {
        DebugPointerCaptureRuntimeCliCommand::OverlayProbe {
            frames,
            interval_ms,
        } => run_pointer_overlay_capture_probe(app_handle, &output_dir, frames, interval_ms, false),
        DebugPointerCaptureRuntimeCliCommand::OverlayProbeReducedMotion {
            frames,
            interval_ms,
        } => run_pointer_overlay_capture_probe(app_handle, &output_dir, frames, interval_ms, true),
    };

    emit_pointer_capture_payload(&output_dir, &payload)
}

fn pointer_capture_output_dir() -> PathBuf {
    std::env::var("MAEKON_DEBUG_POINTER_CAPTURE_OUTPUT_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("maekon-pointer-direct-capture"))
}

fn run_pointer_capture_probe(
    output_dir: &Path,
    frames: u32,
    interval_ms: u64,
) -> serde_json::Value {
    if let Err(error) = std::fs::create_dir_all(output_dir) {
        return json!({
            "ok": false,
            "command": "debug-pointer-capture probe",
            "error": format!("output directory create failed: {error}"),
            "output_dir": output_dir,
        });
    }

    let capture = maekon_vision::capture::ScreenCapture::new();
    let monitors = maekon_vision::capture::ScreenCapture::list_monitors().unwrap_or_default();
    let started = Instant::now();
    let mut captured_frames = Vec::new();
    let mut frames_saved = 0_u32;
    let mut edge_signal_candidate_frames = 0_u32;
    let mut interior_signal_candidate_frames = 0_u32;
    let mut pointer_position_observed_frames = 0_u32;
    let mut pointer_signal_candidate_frames = 0_u32;
    let mut last_error = None;

    for frame_index in 0..frames {
        if frame_index > 0 {
            std::thread::sleep(Duration::from_millis(interval_ms));
        }

        let frame_path =
            output_dir.join(format!("pointer-direct-capture-frame-{frame_index:03}.png"));
        match capture.capture_primary() {
            Ok(image) => {
                let (width, height) = image.dimensions();
                let pointer = current_pointer_position_snapshot(&monitors);
                let local_pointer = pointer.and_then(|position| position.local);
                let signal = signal_stats(&image, local_pointer);
                if local_pointer.is_some() {
                    pointer_position_observed_frames += 1;
                }
                if signal.edge >= EDGE_SIGNAL_MIN_PIXELS {
                    edge_signal_candidate_frames += 1;
                }
                if signal.interior > 0 {
                    interior_signal_candidate_frames += 1;
                }
                if signal.pointer >= POINTER_SIGNAL_MIN_PIXELS {
                    pointer_signal_candidate_frames += 1;
                }

                match image.save(&frame_path) {
                    Ok(()) => {
                        frames_saved += 1;
                        captured_frames.push(json!({
                            "index": frame_index,
                            "path": frame_path,
                            "width": width,
                            "height": height,
                            "signal_candidate_pixels": signal.total,
                            "edge_signal_candidate_pixels": signal.edge,
                            "interior_signal_candidate_pixels": signal.interior,
                            "pointer_signal_candidate_pixels": signal.pointer,
                            "pointer_position": pointer_position_json(pointer),
                        }));
                    }
                    Err(error) => {
                        last_error = Some(format!("frame save failed: {error}"));
                        captured_frames.push(json!({
                            "index": frame_index,
                            "path": frame_path,
                            "width": width,
                            "height": height,
                            "pointer_position": pointer_position_json(pointer),
                            "error": format!("frame save failed: {error}"),
                        }));
                    }
                }
            }
            Err(error) => {
                last_error = Some(error.to_string());
                captured_frames.push(json!({
                    "index": frame_index,
                    "error": error.to_string(),
                }));
            }
        }
    }

    let ok = frames_saved > 0;
    json!({
        "ok": ok,
        "command": "debug-pointer-capture probe",
        "capture_backend": "xcap",
        "backend_contract": "os-screen-capture-not-gdi-copyfromscreen",
        "computer_use_used": false,
        "gdi_copyfromscreen_used": false,
        "frames_requested": frames,
        "frames_saved": frames_saved,
        "interval_ms": interval_ms,
        "duration_ms": started.elapsed().as_millis(),
        "output_dir": output_dir,
        "metadata_path": output_dir.join("pointer-direct-capture-metadata.json"),
        "monitors": monitors,
        "signal_detection": {
            "method": "brand-signal-teal-pixel-heuristic",
            "scan_stride_px": SIGNAL_SCAN_STRIDE_PX,
            "edge_signal_min_pixels": EDGE_SIGNAL_MIN_PIXELS,
            "pointer_signal_min_pixels": POINTER_SIGNAL_MIN_PIXELS,
            "pointer_signal_radius_px": {
                "min": POINTER_SIGNAL_MIN_RADIUS_PX,
                "max": POINTER_SIGNAL_MAX_RADIUS_PX,
            },
            "edge_signal_candidate_frames": edge_signal_candidate_frames,
            "interior_signal_candidate_frames": interior_signal_candidate_frames,
            "pointer_position_observed_frames": pointer_position_observed_frames,
            "pointer_signal_candidate_frames": pointer_signal_candidate_frames,
        },
        "frames": captured_frames,
        "error": if ok { serde_json::Value::Null } else { json!(last_error.unwrap_or_else(|| "no frames saved".to_string())) },
    })
}

fn run_pointer_overlay_capture_probe(
    app_handle: &AppHandle,
    output_dir: &Path,
    frames: u32,
    interval_ms: u64,
    reduced_motion: bool,
) -> serde_json::Value {
    let command_name = if reduced_motion {
        "debug-pointer-capture overlay-probe-reduced-motion"
    } else {
        "debug-pointer-capture overlay-probe"
    };
    let Some(state) = app_handle.try_state::<crate::runtime_state::AppState>() else {
        return json!({
            "ok": false,
            "command": command_name,
            "error": "AppState unavailable for overlay probe",
            "output_dir": output_dir,
        });
    };
    let Some(overlay) = state.magic_overlay.clone() else {
        return json!({
            "ok": false,
            "command": command_name,
            "error": "MagicOverlay unavailable for overlay probe",
            "output_dir": output_dir,
        });
    };

    state.capture_paused.store(false, Ordering::Relaxed);
    state.indicator_visible.store(true, Ordering::Relaxed);
    let capture_state = json!({ "paused": false, "indicator_visible": true });
    let _ = app_handle.emit("overlay:capture-state-changed", &capture_state);
    if let Err(error) = overlay.ensure_window() {
        return json!({
            "ok": false,
            "command": command_name,
            "error": format!("MagicOverlay window unavailable for overlay probe: {error}"),
            "output_dir": output_dir,
        });
    }
    overlay.show_passive_tracking_surface();

    let (pointer_x, pointer_y) = current_global_pointer_position().unwrap_or((960, 540));
    let pointer_payload = |click_count| crate::magic_overlay::OverlayPointerContextPayload {
        enabled: true,
        x: Some(pointer_x.max(0) as f32),
        y: Some(pointer_y.max(0) as f32),
        click_count,
        click_pulse: !reduced_motion,
        reduced_motion,
        ttl_ms: 1_500,
        sample_rate_hz: 30,
    };

    let timing = overlay_probe_timing_contract();
    std::thread::sleep(timing.warmup_before_first_emit);
    for click_count in 1..=timing.emit_attempts {
        overlay.emit_pointer_context(pointer_payload(click_count));
        std::thread::sleep(timing.emit_interval);
    }
    std::thread::sleep(timing.settle_after_last_emit);

    let mut payload = run_pointer_capture_probe(output_dir, frames, interval_ms);
    if let Some(object) = payload.as_object_mut() {
        object.insert("command".to_string(), json!(command_name));
        object.insert("runtime_overlay_probe".to_string(), json!(true));
        object.insert("reduced_motion_probe".to_string(), json!(reduced_motion));
        object.insert(
            "animation_contract".to_string(),
            json!({
                "reduced_motion": reduced_motion,
                "halo_visible_without_animation": reduced_motion,
                "click_ripple_suppressed": reduced_motion,
                "high_contrast_readable": true,
            }),
        );
        object.insert(
            "injected_pointer_context".to_string(),
            json!({
                "x": pointer_x,
                "y": pointer_y,
                "click_pulse": !reduced_motion,
                "reduced_motion": reduced_motion,
                "ttl_ms": 1_500,
            }),
        );
        object.insert(
            "overlay_probe_timing".to_string(),
            json!({
                "webview_warmup_ms": timing.warmup_before_first_emit.as_millis(),
                "pointer_context_emit_attempts": timing.emit_attempts,
                "pointer_context_emit_interval_ms": timing.emit_interval.as_millis(),
                "pre_capture_settle_ms": timing.settle_after_last_emit.as_millis(),
            }),
        );
    }
    payload
}

fn signal_stats(image: &image::DynamicImage, pointer: Option<LocalPointerPosition>) -> SignalStats {
    let rgba = image.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let mut stats = SignalStats::default();

    for y in (0..height).step_by(SIGNAL_SCAN_STRIDE_PX as usize) {
        for x in (0..width).step_by(SIGNAL_SCAN_STRIDE_PX as usize) {
            let [r, g, b, a] = rgba.get_pixel(x, y).0;
            if is_brand_signal_candidate(r, g, b, a) {
                stats.total += 1;
                if is_edge_pixel(x, y, width, height) {
                    stats.edge += 1;
                } else {
                    stats.interior += 1;
                }
                if pointer.is_some_and(|position| is_pointer_signal_pixel(x, y, position)) {
                    stats.pointer += 1;
                }
            }
        }
    }

    stats
}

fn is_brand_signal_candidate(r: u8, g: u8, b: u8, a: u8) -> bool {
    if a < 48 {
        return false;
    }

    let r = i16::from(r);
    let g = i16::from(g);
    let b = i16::from(b);

    g >= 110 && b >= 95 && r <= 105 && g - r >= 45 && b - r >= 35 && (g - b).abs() <= 95
}

fn is_edge_pixel(x: u32, y: u32, width: u32, height: u32) -> bool {
    let edge_band = 8;
    x < edge_band
        || y < edge_band
        || width.saturating_sub(x) <= edge_band
        || height.saturating_sub(y) <= edge_band
}

fn is_pointer_signal_pixel(x: u32, y: u32, pointer: LocalPointerPosition) -> bool {
    let dx = i64::from(x) - i64::from(pointer.x);
    let dy = i64::from(y) - i64::from(pointer.y);
    let distance_squared = dx * dx + dy * dy;
    let min_squared = POINTER_SIGNAL_MIN_RADIUS_PX * POINTER_SIGNAL_MIN_RADIUS_PX;
    let max_squared = POINTER_SIGNAL_MAX_RADIUS_PX * POINTER_SIGNAL_MAX_RADIUS_PX;

    distance_squared >= min_squared && distance_squared <= max_squared
}

fn current_pointer_position_snapshot(
    monitors: &[maekon_vision::capture::MonitorInfo],
) -> Option<PointerPositionSnapshot> {
    let (global_x, global_y) = current_global_pointer_position()?;
    let primary = monitors
        .iter()
        .find(|monitor| monitor.is_primary)
        .or_else(|| monitors.first());
    let local = primary.and_then(|monitor| {
        let local_x = i64::from(global_x) - i64::from(monitor.x);
        let local_y = i64::from(global_y) - i64::from(monitor.y);
        if local_x >= 0
            && local_y >= 0
            && local_x < i64::from(monitor.width)
            && local_y < i64::from(monitor.height)
        {
            Some(LocalPointerPosition {
                x: local_x as u32,
                y: local_y as u32,
            })
        } else {
            None
        }
    });

    Some(PointerPositionSnapshot {
        global_x,
        global_y,
        local,
    })
}

#[cfg(target_os = "windows")]
fn current_global_pointer_position() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: GetCursorPos writes the cursor coordinates through the `&mut point`
    // out-pointer, which references an initialized stack `POINT` that outlives
    // the call; there is no ownership transfer. A zero return simply means no
    // pointer position is available.
    let ok = unsafe { GetCursorPos(&mut point) };
    (ok != 0).then_some((point.x, point.y))
}

#[cfg(not(target_os = "windows"))]
fn current_global_pointer_position() -> Option<(i32, i32)> {
    None
}

fn pointer_position_json(pointer: Option<PointerPositionSnapshot>) -> serde_json::Value {
    match pointer {
        Some(pointer) => json!({
            "global_x": pointer.global_x,
            "global_y": pointer.global_y,
            "local_x": pointer.local.map(|local| local.x),
            "local_y": pointer.local.map(|local| local.y),
            "on_primary": pointer.local.is_some(),
        }),
        None => serde_json::Value::Null,
    }
}

fn emit_pointer_capture_payload(output_dir: &Path, payload: &serde_json::Value) -> i32 {
    let line = match serde_json::to_string(payload) {
        Ok(line) => line,
        Err(error) => {
            eprintln!("debug-pointer-capture serialization failed: {error}");
            return 1;
        }
    };
    println!("{line}");

    if let Err(error) = std::fs::create_dir_all(output_dir) {
        eprintln!("debug-pointer-capture output directory create failed: {error}");
        return 1;
    }

    let metadata_path = output_dir.join("pointer-direct-capture-metadata.json");
    if let Err(error) = std::fs::write(&metadata_path, format!("{line}\n")) {
        eprintln!(
            "debug-pointer-capture metadata write failed at {}: {error}",
            metadata_path.display()
        );
        return 1;
    }

    if payload.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_signal_candidate_accepts_overlay_teal() {
        assert!(is_brand_signal_candidate(20, 184, 166, 255));
        assert!(is_brand_signal_candidate(45, 212, 191, 255));
    }

    #[test]
    fn brand_signal_candidate_rejects_neutral_pixels() {
        assert!(!is_brand_signal_candidate(255, 255, 255, 255));
        assert!(!is_brand_signal_candidate(30, 30, 30, 255));
        assert!(!is_brand_signal_candidate(20, 184, 166, 20));
    }

    #[test]
    fn pointer_signal_candidate_accepts_localized_halo_annulus() {
        let mut image = image::RgbaImage::from_pixel(120, 120, image::Rgba([255, 255, 255, 255]));
        for offset in [-24_i32, -22, -20, 20, 22, 24] {
            image.put_pixel((60 + offset) as u32, 60, image::Rgba([20, 184, 166, 255]));
            image.put_pixel(60, (60 + offset) as u32, image::Rgba([20, 184, 166, 255]));
        }

        let stats = signal_stats(
            &image::DynamicImage::ImageRgba8(image),
            Some(LocalPointerPosition { x: 60, y: 60 }),
        );

        assert!(stats.pointer >= POINTER_SIGNAL_MIN_PIXELS);
    }

    #[test]
    fn pointer_signal_candidate_rejects_generic_teal_far_from_pointer() {
        let mut image = image::RgbaImage::from_pixel(120, 120, image::Rgba([255, 255, 255, 255]));
        for y in 90..110 {
            for x in 90..110 {
                image.put_pixel(x, y, image::Rgba([20, 184, 166, 255]));
            }
        }

        let stats = signal_stats(
            &image::DynamicImage::ImageRgba8(image),
            Some(LocalPointerPosition { x: 30, y: 30 }),
        );

        assert!(stats.interior > 0);
        assert_eq!(stats.pointer, 0);
    }

    #[test]
    fn overlay_probe_timing_contract_allows_webview_listener_startup() {
        let contract = overlay_probe_timing_contract();

        assert!(
            contract.warmup_before_first_emit >= Duration::from_millis(1_000),
            "overlay probe should leave enough time for the hidden WebView to load listeners"
        );
        assert!(
            contract.emit_attempts >= 5,
            "overlay probe should re-emit pointer context because Tauri events are not buffered"
        );
        assert!(
            contract.settle_after_last_emit >= Duration::from_millis(250),
            "overlay probe should let React paint the pointer halo before capture starts"
        );
    }
}
