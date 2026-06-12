use maekon_core::models::focused_element::FocusedElementInfo;
use maekon_core::models::gui::{HighlightRequest, HighlightTarget};
use maekon_core::models::intent::ElementBounds;
use maekon_core::ports::element_finder::ElementFinder;
use maekon_core::ports::overlay_driver::OverlayDriver;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::debug;

// OCR 재분석 동시 실행 상한선: 1
//
// 1초 모니터 틱마다 앱/타이틀 변경 시 OCR 작업을 spawn하면 빠른 창 전환 시
// 태스크가 무한 누적된다. 가장 최신 씬만 의미있으므로 cap=1로 나머지는 skip.
// segment.rs LLM_SUMMARY_SEMAPHORE(cap=4) 와 동일한 const_new + RAII 패턴 적용.
pub(super) static DETECTION_OCR_SEMAPHORE: Semaphore = Semaphore::const_new(1);

use crate::magic_overlay::MagicOverlayHandle;

// ── Focus highlight state carried across ticks ──────────────────────

/// Debounce threshold: element must remain stable for this many consecutive
/// ticks before the highlight overlay is updated.
const DEBOUNCE_TICKS: u32 = 2;

/// Mutable state for focus-highlight debouncing.
pub(super) struct FocusHighlightState {
    /// Currently displayed element key (role, label).
    pub prev_key: Option<(String, String)>,
    pub last_handle_id: Option<String>,
    /// Candidate element key waiting to stabilize.
    pending_key: Option<(String, String)>,
    /// How many consecutive ticks the pending_key has been the same.
    stable_ticks: u32,
}

impl FocusHighlightState {
    pub fn new() -> Self {
        Self {
            prev_key: None,
            last_handle_id: None,
            pending_key: None,
            stable_ticks: 0,
        }
    }
}

// ── Focus highlight update ──────────────────────────────────────────

/// Update the focus highlight overlay after an accessibility extraction
/// succeeds.  Returns the new `FocusedElementInfo` to store.
pub(super) async fn update_focus_highlight(
    info: Option<FocusedElementInfo>,
    state: &mut FocusHighlightState,
    overlay_driver: &Option<Arc<dyn OverlayDriver>>,
) -> Option<FocusedElementInfo> {
    let current_key = info.as_ref().and_then(|fe| {
        fe.position.filter(|p| p.width > 0.0 && p.height > 0.0)?;
        Some((fe.role.clone(), fe.label.clone().unwrap_or_default()))
    });

    // Debounce: require element to be stable for DEBOUNCE_TICKS before updating overlay
    if current_key == state.prev_key {
        // Already displaying this element — reset pending
        state.pending_key = None;
        state.stable_ticks = 0;
        return info;
    }

    if current_key == state.pending_key {
        state.stable_ticks += 1;
    } else {
        state.pending_key = current_key.clone();
        state.stable_ticks = 1;
    }

    if state.stable_ticks < DEBOUNCE_TICKS {
        // Not yet stable — don't update overlay
        return info;
    }

    // Element has been stable for enough ticks — update overlay
    if let Some(ref driver) = overlay_driver {
        // Clear previous highlight first
        if let Some(ref prev_id) = state.last_handle_id {
            if let Err(e) = driver.clear_highlights(prev_id).await {
                debug!("focus highlight clear failed: {e}");
            }
            state.last_handle_id = None;
        }

        // Show new highlight if element has valid bounds
        if let Some(ref fe) = info {
            if let Some(ref pos) = fe.position {
                if pos.width > 0.0 && pos.height > 0.0 {
                    let req = HighlightRequest {
                        session_id: String::new(),
                        scene_id: String::new(),
                        targets: vec![HighlightTarget {
                            candidate_id: "ax-focus".to_string(),
                            bbox_abs: ElementBounds {
                                x: pos.x as i32,
                                y: pos.y as i32,
                                width: pos.width as u32,
                                height: pos.height as u32,
                            },
                            color: "#0d9488".to_string(),
                            label: fe.label.clone(),
                        }],
                    };
                    match driver.show_highlights(req).await {
                        Ok(handle) => {
                            state.last_handle_id = Some(handle.handle_id);
                        }
                        Err(e) => {
                            debug!("focus highlight show failed: {e}");
                        }
                    }
                }
            }
        }
    }
    state.prev_key = current_key;
    state.pending_key = None;
    state.stable_ticks = 0;

    info
}

/// Clear highlight state when accessibility extraction fails.
pub(super) async fn clear_focus_highlight(
    state: &mut FocusHighlightState,
    overlay_driver: &Option<Arc<dyn OverlayDriver>>,
) {
    if state.prev_key.is_some() {
        if let Some(ref driver) = overlay_driver {
            if let Some(ref prev_id) = state.last_handle_id {
                if let Err(e) = driver.clear_highlights(prev_id).await {
                    debug!("clear_highlights failed: {e}");
                }
            }
        }
        state.last_handle_id = None;
        state.prev_key = None;
    }
}

// ── Detection re-analysis ───────────────────────────────────────────

/// Re-analyze the current scene when the detection overlay is active and
/// the foreground app or window title changed. Spawns the analysis in a
/// background task so the monitor loop is not blocked.
pub(super) fn maybe_reanalyze_detection(
    detection_active: &AtomicBool,
    app_changed: bool,
    title_changed: bool,
    scene_finder: &Option<Arc<dyn ElementFinder>>,
    magic_overlay: &Option<MagicOverlayHandle>,
) {
    if !detection_active.load(Ordering::Relaxed) {
        return;
    }
    if !app_changed && !title_changed {
        return;
    }
    if let (Some(finder), Some(overlay)) = (scene_finder, magic_overlay) {
        // DETECTION_OCR_SEMAPHORE: 동시 OCR 1개로 제한.
        // 이미 분석 중이면 현재 틱을 건너뜀 — 최신 씬만 의미있기 때문.
        match DETECTION_OCR_SEMAPHORE.try_acquire() {
            Ok(permit) => {
                let finder = finder.clone();
                let overlay = overlay.clone();
                tokio::spawn(async move {
                    // permit은 이 클로저가 종료될 때 자동 해제됨 (RAII).
                    let _permit = permit;
                    match finder.analyze_scene(None, None).await {
                        Ok(scene) => {
                            overlay.emit_detection_scene(&scene).await;
                        }
                        Err(e) => {
                            debug!("detection re-analysis on window change failed: {e}");
                        }
                    }
                });
            }
            Err(_) => {
                // 이미 OCR 진행 중 — 이전 분석이 완료되면 최신 씬으로 재시도됨.
                debug!("DETECTION_OCR_SEMAPHORE exhausted, skipping reanalysis");
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::DETECTION_OCR_SEMAPHORE;

    /// DETECTION_OCR_SEMAPHORE cap=1 검증.
    ///
    /// - permit 1개 획득 후 2번째 try_acquire는 Err를 반환해야 함 (skip 경로 진입).
    /// - 첫 번째 permit drop 후 재획득 가능해야 함 (RAII 정상 해제 확인).
    /// segment.rs llm_summary_semaphore_caps_at_four 와 동일한 패턴.
    #[tokio::test]
    async fn test_detection_ocr_semaphore_skips_concurrent() {
        // cap=1 이므로 첫 번째 permit 획득 성공.
        let p1 = DETECTION_OCR_SEMAPHORE
            .try_acquire()
            .expect("첫 번째 permit 획득 실패 — cap=1 세마포어가 비어있어야 함");

        // 두 번째 획득은 실패해야 함 (OCR skip 경로).
        assert!(
            matches!(
                DETECTION_OCR_SEMAPHORE.try_acquire(),
                Err(tokio::sync::TryAcquireError::NoPermits)
            ),
            "세마포어가 소진된 상태에서 try_acquire는 TryAcquireError::NoPermits 를 반환해야 함"
        );

        // permit drop → RAII 해제 후 재획득 가능.
        drop(p1);

        // Pin: after the permit is dropped the next try_acquire must succeed,
        // proving RAII release restores the cap-1 semaphore to a free state.
        let _reacquired = DETECTION_OCR_SEMAPHORE
            .try_acquire()
            .expect("semaphore must have a free slot after permit is dropped (RAII release)");
    }
}
