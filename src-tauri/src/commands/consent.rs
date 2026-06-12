//! Tauri IPC: production consent read/write (GDPR).
//!
//! Phase A.2 UI 토글이 동의를 부여하고 `Arc<dyn ConsentManagerPort>`를 통해
//! 스케줄러에서 즉시 관찰 가능하도록 하는 3개의 IPC 커맨드를 제공한다.
//!
//! # GDPR Art. 17 로컬 데이터 삭제 (#4801)
//!
//! `withdraw_consent`는 revoke persist 이후 로컬 데이터를 2단계로 삭제한다:
//! - Phase-1: SQLite 전체 테이블 원자적 삭제 (`delete_all_data`).
//! - Phase-2: 프레임 이미지 파일 삭제 (`delete_all_frames`).
//!
//! Phase-2 실패 시 `pending_local_erase=1` 마커를 `app_meta`에 기록하여
//! 다음 앱 기동 시 `retry_pending_local_erase`가 재시도하도록 한다 (R2/R3).
//!
//! 원격 전파(DeletionEvent)는 단독 소유자인 sync_engine에 위임한다 (R4).
use std::sync::Arc;

use maekon_core::consent::{ConsentPermissions, ConsentStatus};
use maekon_core::models::audit::{AuditEntry, AuditStatus};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_storage::sqlite::SqliteStorage;
use serde::{Deserialize, Serialize};
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

/// 동의 보존 정책 일수 (스토리지 보존 정책과 일치; `expires_at`은 None 유지).
const RETENTION_DAYS: u32 = 30;

/// #4686: 마이크 업그레이드 1회 알림을 이미 보였는지 기록하는 `app_meta` 키.
const MIC_UPGRADE_NOTICE_FLAG: &str = "microphone_split_notice_shown";

/// #4801: 프레임 파일 삭제가 완료되지 않았음을 재시작 후에도 알리는 `app_meta` 키.
///
/// Phase-2(프레임 삭제) 실패 시 이 마커를 `set_meta_checked`로 기록한다.
/// 다음 앱 기동 시 `retry_pending_local_erase`가 마커를 감지하고 삭제를 재시도한다.
/// 양 단계 모두 성공한 뒤 `delete_meta_checked`로 마커를 지운다.
pub(crate) const PENDING_LOCAL_ERASE_KEY: &str = "pending_local_erase";

/// 현재 동의 상태의 스냅샷 DTO.
///
/// 프론트엔드에 상태(`status`)와 허가 집합(`permissions`)을 한 번에 전달한다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentSnapshot {
    /// 유효성 상태 (Valid / NotGranted / Expired / UpdateRequired).
    pub status: ConsentStatus,
    /// 현재 부여된 권한 집합.
    pub permissions: ConsentPermissions,
}

// ---------------------------------------------------------------------------
// 순수 헬퍼 — AppState 없이 테스트 가능한 단위 로직
// ---------------------------------------------------------------------------

/// `ConsentManager`에서 현재 상태를 읽어 `ConsentSnapshot`을 반환한다.
///
/// `status_and_permissions()`로 상태와 권한을 **단일 read 가드** 안에서 함께
/// 읽으므로, 두 값이 서로 다른 시점을 가리키는 TOCTOU 창이 없다 (F5). 권한은
/// 비-Valid 상태에서도 0으로 마스킹하지 않은 **원본 부여 권한**이다 — UI는 상태
/// (예: Expired)와 함께 "무엇이 부여되었는지"를 보여줘야 하기 때문이다.
pub(crate) fn read_consent_snapshot(cm: &dyn ConsentManagerPort) -> ConsentSnapshot {
    let (status, permissions) = cm.status_and_permissions();
    ConsentSnapshot {
        status,
        permissions,
    }
}

/// #4686: 마이크 업그레이드 1회 알림을 표시해야 하는지 판정하는 순수 함수.
///
/// `audio.enabled`(오디오 캡처 ON)인데 `microphone` 동의가 아직 없고(#4568 분리 이후
/// 기본 OFF) 알림을 아직 보인 적 없으면 `true`. AppState 없이 단위 테스트 가능하다.
fn should_show_microphone_upgrade_notice(
    audio_enabled: bool,
    microphone_granted: bool,
    already_shown: bool,
) -> bool {
    audio_enabled && !microphone_granted && !already_shown
}

/// 감사 로그 항목을 SQLite에 기록한다.
///
/// - `action`: `"consent_granted"` 또는 `"consent_revoked"`
/// - `consent_id`: 부여 시 ConsentRecord.consent_id; 철회 시 빈 문자열.
fn audit_consent(
    storage: &SqliteStorage,
    action: &str,
    perms: &ConsentPermissions,
    consent_id: &str,
) {
    // details 필드에 결과 권한 집합·정책 버전·consent_id를 JSON으로 기록한다.
    let details = serde_json::json!({
        "permissions": perms,
        "version": maekon_core::consent::CURRENT_POLICY_VERSION,
        "consent_id": consent_id,
    });

    storage.save_audit_entry(&AuditEntry {
        entry_id: maekon_core::id_generation::generate_id("audit"),
        timestamp: chrono::Utc::now(),
        // #4685: a consent change is a system-level event not tied to any tracking
        // session or automation command. Use a distinct "system.consent" sentinel
        // (not the bare "consent" string, which collided with per-session audit
        // correlation by reusing the same value for both session_id and command_id).
        session_id: "system.consent".into(),
        command_id: "system.consent".into(),
        action_type: action.into(),
        // 성공 처리 — Granted/Revoked 변형이 없으므로 Completed 사용.
        status: AuditStatus::Completed,
        details: Some(details.to_string()),
        execution_time_ms: None,
    });
}

/// 동의 부여 + 감사 기록을 수행하고 결과 스냅샷을 반환한다.
///
/// `grant_consent` 파일 I/O 실패는 `IpcError`로 변환해 호출자에게 전파한다.
/// 감사 로그는 파일 쓰기가 성공한 이후에만 기록한다 — 실패한 부여는 감사하지 않는다.
pub(crate) fn apply_set_consent(
    cm: &dyn ConsentManagerPort,
    storage: &SqliteStorage,
    permissions: ConsentPermissions,
) -> Result<ConsentSnapshot, IpcError> {
    // 파일 I/O 실패 시 Err를 즉시 반환한다 (GDPR 준수: persist 전 Ok 반환 금지).
    cm.grant_consent(permissions.clone(), RETENTION_DAYS)
        .map_err(IpcError::from)?;
    let consent_id = cm
        .current_consent()
        .map(|r| r.consent_id)
        .unwrap_or_default();
    // persist 성공 후에만 감사 기록한다.
    audit_consent(storage, "consent_granted", &permissions, &consent_id);
    Ok(read_consent_snapshot(cm))
}

/// 동의 철회 + 감사 기록을 수행하고 결과 스냅샷을 반환한다.
///
/// `revoke_consent` 파일 I/O 실패는 `IpcError`로 변환해 호출자에게 전파한다.
/// 철회 실패 시 UI에 Ok를 반환하면 GDPR Art. 7§3(철회권) 위반이므로 반드시 전파한다.
/// 감사 로그는 철회가 성공한 이후에만 기록한다.
pub(crate) fn apply_withdraw_consent(
    cm: &dyn ConsentManagerPort,
    storage: &SqliteStorage,
) -> Result<ConsentSnapshot, IpcError> {
    // 철회 전 권한 집합·consent_id를 캡처해 감사 로그에 "무엇이 철회되었는지"를 남긴다
    // (GDPR Art. 7§1 demonstrability — 철회 이벤트만으로 철회된 권한을 재구성할 수 있어야 한다).
    let prior = cm.current_consent();
    let prior_permissions = prior
        .as_ref()
        .map(|r| r.permissions.clone())
        .unwrap_or_default();
    let prior_consent_id = prior.map(|r| r.consent_id).unwrap_or_default();
    // 파일 I/O 실패 시 Err를 즉시 반환한다 (철회 persist 실패를 Ok로 감추는 것은 GDPR 위반).
    cm.revoke_consent().map_err(IpcError::from)?;
    // persist 성공 후에만 감사 기록한다. 철회된(직전) 권한 집합을 기록한다 — all-false 기본값이 아니라.
    audit_consent(
        storage,
        "consent_revoked",
        &prior_permissions,
        &prior_consent_id,
    );
    Ok(read_consent_snapshot(cm))
}

/// #5056: consent-change → telemetry exporter re-apply (consent→exporter bridge).
///
/// After a consent grant/revoke succeeds, re-evaluate the consent gate and push
/// the result to the telemetry `Handle` immediately, rather than waiting for the
/// next config change. This is what makes a telemetry-consent revoke shut the
/// OTLP exporter down at once, and a grant (with config already enabled) start
/// it.
///
/// Reads the live state through `AppHandle`:
///   - `Arc<telemetry::Handle>` (managed in `main.rs`),
///   - `ConfigRuntimeState` → current `config.telemetry` (the raw user setting),
///   - `AppState.capture.consent_manager` → the live consent record.
///
/// Fail-closed by construction: the gate is computed via
/// `consent_gated_telemetry_config`, which reads consent through the fail-closed
/// `effective_permissions()` accessor — any non-Valid status (absent / revoked /
/// expired) collapses the telemetry term to false ⇒ exporter OFF.
///
/// Best-effort: any missing managed state or `Handle::apply` error is logged and
/// swallowed — a telemetry re-apply failure MUST NOT fail the consent write
/// (GDPR Art. 7§3 / Art. 6: the consent change itself already persisted). Under
/// the no-op telemetry shim (`--no-default-features`) `apply` is an infallible
/// no-op, so this is a cheap, safe call there too.
pub(crate) fn reapply_telemetry_gate(app: &tauri::AppHandle) {
    use tauri::Manager;

    // Telemetry handle — managed in main.rs. Absent only in degraded builds.
    let Some(handle) = app.try_state::<Arc<crate::telemetry::Handle>>() else {
        tracing::warn!("telemetry gate re-apply skipped: no telemetry Handle in managed state");
        return;
    };

    // Live raw telemetry config from the ConfigRuntimeState.
    let Some(config_state) = app.try_state::<crate::runtime_state::ConfigRuntimeState>() else {
        tracing::warn!("telemetry gate re-apply skipped: no ConfigRuntimeState in managed state");
        return;
    };
    let telemetry_config = config_state.config_manager().get().telemetry;

    // Live consent manager from AppState's capture context.
    let Some(app_state) = app.try_state::<AppState>() else {
        tracing::warn!("telemetry gate re-apply skipped: no AppState in managed state");
        return;
    };
    let Some(consent_manager) = app_state.capture.consent_manager.as_ref() else {
        // No consent wired → fail-closed: drive the exporter OFF explicitly so a
        // stale enabled state cannot linger.
        tracing::warn!(
            "telemetry gate re-apply: no ConsentManager wired — forcing exporter OFF (fail-closed)"
        );
        let off = maekon_core::config::TelemetryConfig {
            enabled: false,
            ..telemetry_config
        };
        if let Err(e) = handle.apply(&off) {
            tracing::warn!(error = %e, "telemetry gate re-apply (forced OFF) failed");
        }
        return;
    };

    // Compute the consent-gated config and apply it. apply() is idempotent when
    // the gated config matches the last-applied value.
    let gated = crate::telemetry::consent_gated_telemetry_config(
        &telemetry_config,
        consent_manager.as_ref(),
    );
    if let Err(e) = handle.apply(&gated) {
        tracing::warn!(error = %e, "telemetry gate re-apply failed");
    } else {
        tracing::debug!(
            telemetry_enabled = gated.enabled,
            "telemetry gate re-applied after consent change"
        );
    }
}

/// `AppState`에서 `Arc<dyn ConsentManagerPort>`를 추출한다.
///
/// `capture.consent_manager`가 None이면 `service.unavailable` IpcError를 반환한다.
fn consent_mgr(state: &AppState) -> Result<&Arc<dyn ConsentManagerPort>, IpcError> {
    state
        .capture
        .consent_manager
        .as_ref()
        .ok_or_else(|| IpcError::new("service.unavailable", "consent manager not available"))
}

// ---------------------------------------------------------------------------
// Tauri IPC 커맨드
// ---------------------------------------------------------------------------

/// 현재 동의 상태를 읽어 반환한다.
///
/// 쓰기 없이 읽기만 수행하므로 `&self` ConsentManager로 충분하다.
#[command]
pub async fn get_consent(state: tauri::State<'_, AppState>) -> Result<ConsentSnapshot, IpcError> {
    Ok(read_consent_snapshot(consent_mgr(&state)?.as_ref()))
}

/// 지정된 권한 집합으로 동의를 부여하고, 감사 로그를 기록한 후 결과 스냅샷을 반환한다.
///
/// `apply_set_consent`는 `grant_consent`(blocking `std::fs::write`) +
/// `save_audit_entry`(blocking SQLite)를 수행하는 **동기** 헬퍼이므로,
/// async 워커 풀이 굶지 않도록 `spawn_blocking`으로 옮긴다 (F-RR-06 async-safety).
///
/// 동의 쓰기 직후 VAD 재게이트 신호를 발사한다 (F-MIC-2, #4568): `microphone`을
/// 철회/축소하면 실행 중인 VAD 리스너가 ≤2 s 백스톱 틱을 기다리지 않고 즉시
/// 마이크를 내린다. **bare `signal_vad_regate`** 를 사용한다 — `on_capture_pause_toggled`
/// 는 unpause 엣지에서 VAD 를 자동 재무장하므로, 동의 *부여* 시 마이크가 자동
/// 시작되는 (consent-to-surveillance) 에스컬레이션을 피한다. 어떤 동의 쓰기든
/// 무조건 발사한다 (게이트가 여전히 열려 있으면 수신자는 no-op — idempotent).
/// 신호는 blocking 가드가 해제된 뒤 발사되는 non-blocking `notify_one` 이다.
#[command]
pub async fn set_consent(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    permissions: ConsentPermissions,
) -> Result<ConsentSnapshot, IpcError> {
    // Arc를 복제해 'static 클로저로 소유권을 넘긴다 (State borrow는 await를 넘길 수 없다).
    let cm = consent_mgr(&state)?.clone();
    let storage = state.storage.clone();
    let snapshot =
        tokio::task::spawn_blocking(move || apply_set_consent(cm.as_ref(), &storage, permissions))
            .await
            .map_err(|join_err| {
                IpcError::new(
                    "internal.generic",
                    format!("set_consent task join failed: {join_err}"),
                )
            })??;
    // persist 성공 후 (blocking 가드 해제 후) VAD 재게이트 신호 발사.
    crate::commands::audio::signal_vad_regate(&app);
    // #5056: re-evaluate the consent gate on the telemetry exporter. Granting
    // telemetry consent (with config already enabled) starts the exporter
    // immediately; any other grant is a no-op re-apply. Best-effort — never
    // fails the consent write.
    reapply_telemetry_gate(&app);
    Ok(snapshot)
}

/// 동의를 철회하고, 감사 로그를 기록한 후 결과 스냅샷을 반환한다.
///
/// `apply_withdraw_consent`는 `revoke_consent`(blocking write+rename, write 가드를
/// 가로질러 보유) + `save_audit_entry`(blocking SQLite)를 수행하는 **동기** 헬퍼이므로,
/// async 워커 풀이 굶지 않도록 `spawn_blocking`으로 옮긴다 (F-RR-06 async-safety).
/// revoke의 단일-가드 원자성 설계는 헬퍼 내부에서 그대로 유지된다 (여기선 풀만 분리).
///
/// # GDPR Art. 17 로컬 삭제 (#4801, Decision A: 전체-삭제)
///
/// revoke persist 성공 직후 로컬 데이터를 2단계로 삭제한다:
/// - Phase-1: SQLite 전체 테이블 원자적 삭제. 실패 시 Err 반환 (R5).
/// - Phase-2: 프레임 이미지 파일 삭제. 실패 시 재시도 마커 기록 + 부분 상태 표시 (R2/R3).
///
/// 원격 DeletionEvent 전파는 sync_engine에 위임한다 (R4: pending_deletion=true 유지).
///
/// 철회 persist 직후 VAD 재게이트 신호를 발사한다 (F-MIC-2, #4568): 실행 중인 VAD
/// 리스너가 즉시 마이크를 내리도록 한다. `set_consent` 와 동일하게 bare
/// `signal_vad_regate` 를 blocking 가드 해제 후 발사한다.
#[command]
pub async fn withdraw_consent(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ConsentSnapshot, IpcError> {
    let cm = consent_mgr(&state)?.clone();
    let storage = state.storage.clone();

    // Step 1: revoke persist + 감사 로그 (blocking I/O → spawn_blocking).
    let snapshot =
        tokio::task::spawn_blocking(move || apply_withdraw_consent(cm.as_ref(), &storage))
            .await
            .map_err(|join_err| {
                IpcError::new(
                    "internal.generic",
                    format!("withdraw_consent task join failed: {join_err}"),
                )
            })??;

    // revoke persist 성공 후 VAD 재게이트 신호 발사 (F-MIC-2).
    // 신호 발사는 새 캡처를 즉시 차단하는 효과를 낸다 (is_permitted = false).
    crate::commands::audio::signal_vad_regate(&app);
    // #5056: re-evaluate the consent gate on the telemetry exporter. Revoking
    // consent zeroes `effective_permissions().telemetry`, so this drives the
    // OTLP exporter OFF immediately (fail-closed) rather than waiting for the
    // next config change. Best-effort — never fails the revoke.
    reapply_telemetry_gate(&app);

    // Step 2: GDPR Art. 17 로컬 데이터 삭제 (R5: 실패 시 Ok 반환 불가).
    let frame_storage = state.capture.frame_storage.clone();
    let storage = state.storage.clone();
    erase_all_local_data(storage, frame_storage).await?;

    Ok(snapshot)
}

/// #4928 round-3 (FIX B): erase 윈도우 동안 `erasing` 신호를 보유하는 RAII 가드.
///
/// `set` 시점에 공유 `erasing` Arc 를 `true` 로 만들고, `Drop` 시 `false` 로 되돌린다.
/// 이렇게 하면 erase 의 모든 종료 경로(Phase-1 Err, Phase-2 Err, 정상 완료,
/// 패닉-unwind)에서 신호가 반드시 clear 된다 — happy-path 에서만 수동 clear 하는
/// 누락 위험이 없다. `grant_consent` 는 `erasing` 을 건드리지 못하므로, 이 가드가
/// 살아있는 동안에는 재동의가 와도 쓰기가 재개되지 않는다.
struct EraseWindowGuard {
    erasing: Arc<std::sync::atomic::AtomicBool>,
}

impl EraseWindowGuard {
    fn set(erasing: Arc<std::sync::atomic::AtomicBool>) -> Self {
        erasing.store(true, std::sync::atomic::Ordering::Release);
        Self { erasing }
    }
}

impl Drop for EraseWindowGuard {
    fn drop(&mut self) {
        // 모든 종료 경로에서 erase 윈도우를 닫는다(성공/에러/패닉 공통).
        self.erasing
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

/// GDPR Art. 17 로컬 데이터 삭제를 수행한다 (#4801).
///
/// # 순서 (R6)
/// 1. Phase-1: SQLite 전체 사용자 데이터 테이블 원자적 삭제.
///    - 실패 시 `IpcError`를 즉시 반환 (R5: 잔존 SQLite 데이터로 Ok 보고 금지).
/// 2. Phase-2: 프레임 이미지 파일 삭제 (frame_storage가 Some인 경우).
///    - 실패 시 `pending_local_erase=1` 마커를 `set_meta_checked`로 기록 (R2/R3).
///    - 마커 기록 성공 여부와 무관하게 호출자에게 오류를 알린다 (R5).
///
/// # 원격 전파 (R4)
/// `pending_deletion=true`를 건드리지 않는다 — sync_engine이 단독 소유자다.
async fn erase_all_local_data(
    storage: Arc<SqliteStorage>,
    frame_storage: Option<Arc<dyn FrameStoragePort>>,
) -> Result<(), IpcError> {
    // #4928 round-3 (FIX B): grant_consent-during-erase TOCTOU 차단.
    //
    // `deletion_flag` 는 erase 도중 재동의(`grant_consent`)가 `false` 로 되돌릴 수
    // 있으나, `erasing` 은 erase 만 set/clear 하고 `grant_consent` 는 건드리지 못한다.
    // erase 전체 구간(Phase-1 + Phase-2)에 set 해두면, Phase-1 커밋 후 ~ Phase-2 진행
    // 중에 재동의가 끼어들어도 in-flight writer 가 `deletion_flag || erasing` 스킵 술어
    // 때문에 wipe 이후 잔존 행을 쓰지 못한다.
    //
    // `storage.erasing()` 은 composition root 에서 SQLite/frames/ConsentManager 셋이
    // 공유하는 동일 `Arc` 이므로, 이 한 핸들을 set/clear 하면 양쪽 funnel 이 모두 본다.
    // RAII 가드(`EraseWindowGuard`)가 모든 종료 경로(성공/Err/패닉-unwind)에서 clear 한다.
    let _erase_window = EraseWindowGuard::set(storage.erasing());

    // ── Phase-1: SQLite 원자적 삭제 ──────────────────────────────────────────
    // `delete_all_data`는 blocking I/O이므로 spawn_blocking으로 격리한다.
    let storage_clone = storage.clone();
    tokio::task::spawn_blocking(move || storage_clone.delete_all_data())
        .await
        .map_err(|join_err| {
            IpcError::new(
                "internal.generic",
                format!("GDPR SQLite 삭제 태스크 join 실패: {join_err}"),
            )
        })?
        .map_err(|e| {
            // R5: SQLite 삭제 실패 시 Ok 반환 금지 — Err를 즉시 전파한다.
            tracing::error!(err = %e, "GDPR Art.17: SQLite 전체 삭제 실패 — 잔존 사용자 데이터");
            IpcError::from(e)
        })?;

    tracing::info!("GDPR Art.17 Phase-1 완료: SQLite 전체 삭제 성공");

    // ── Phase-2: 프레임 이미지 파일 삭제 ─────────────────────────────────────
    let Some(fs) = frame_storage else {
        // 프레임 스토리지가 없는 경우(오프라인/테스트 환경) — Phase-2 생략.
        return Ok(());
    };

    match fs.delete_all_frames().await {
        Ok(count) => {
            tracing::info!(count, "GDPR Art.17 Phase-2 완료: 프레임 파일 삭제 성공");
            // Phase-2 성공 시 재시도 마커가 있다면 지운다 (이전 부분 삭제 회복).
            if let Err(e) = storage.delete_meta_checked(PENDING_LOCAL_ERASE_KEY) {
                // 마커 삭제 실패는 허용: 다음 기동에서 재시도가 다시 발생할 뿐이다.
                tracing::warn!(err = %e, "GDPR: 재시도 마커 삭제 실패 (non-fatal)");
            }
            Ok(())
        }
        Err(e) => {
            // R3/R5: 프레임 삭제 실패 → 재시도 마커 기록 + 호출자에게 부분 상태 표시.
            tracing::error!(err = %e, "GDPR Art.17 Phase-2 실패: 프레임 이미지 삭제 불완전");
            if let Err(meta_err) = storage.set_meta_checked(PENDING_LOCAL_ERASE_KEY, "1") {
                // 마커 기록도 실패하면 추가 경고 (R2 달성 불가 — 한계 기록).
                tracing::error!(
                    err = %meta_err,
                    "GDPR: 재시도 마커 기록 실패 — 재시작 후 자동 재시도 불가"
                );
            }
            // 프레임 삭제 오류를 IpcError로 변환해 반환한다 (R5).
            Err(IpcError::from(e))
        }
    }
}

/// 앱 기동 시 미완료 로컬 삭제를 재시도한다 (#4801, R2/R3 — 재시작 내구성).
///
/// `app_meta`에 `pending_local_erase=1` 마커가 있으면 `erase_all_local_data`를
/// 재실행한다. 양 단계 성공 후 마커를 `delete_meta_checked`로 삭제한다.
/// 실패해도 다음 기동에서 재시도하므로 앱 시작을 중단하지 않는다 (best-effort 재시도).
///
/// # 호출 위치
/// `app_runtime_launch::mod.rs`의 `build_and_spawn` 함수에서 SQLite가 준비된 직후,
/// 캡처 서비스가 연결되기 전에 호출한다.
pub(crate) async fn retry_pending_local_erase(
    storage: Arc<SqliteStorage>,
    frame_storage: Option<Arc<dyn FrameStoragePort>>,
) {
    // 마커가 없으면 즉시 반환 (정상 기동 경로).
    if storage.get_meta(PENDING_LOCAL_ERASE_KEY).is_none() {
        return;
    }

    tracing::warn!("GDPR Art.17: 미완료 로컬 삭제 마커 감지 — 재시도를 시작한다");

    match erase_all_local_data(storage.clone(), frame_storage).await {
        Ok(()) => {
            tracing::info!("GDPR Art.17: 재시도 성공 — 재시도 마커 삭제");
            // Phase-2가 성공했으므로 마커를 삭제한다 (erase_all_local_data 내부에서 이미 처리).
        }
        Err(e) => {
            // 재시도도 실패 — 다음 기동에서 다시 시도한다 (마커는 유지됨).
            tracing::error!(
                err = %e,
                "GDPR Art.17: 재시도 실패 — 다음 기동에서 재시도 예정"
            );
        }
    }
}

/// #4686: 마이크 분리(#4568) 이후 조용히 멈춘 마이크에 대한 **1회성** 업그레이드 알림.
///
/// `audio.enabled`(사용자가 오디오 캡처를 켬)이지만 `microphone` 동의가 없고(분리 이후
/// 기본 OFF) 아직 알림을 보인 적 없으면 `true`를 **딱 한 번** 반환하고 `app_meta` 플래그를
/// 기록한다. 프론트엔드는 mount 시 이를 호출해 privacy 페이지로 안내하는 1회성 배너를
/// 띄운다. 이후 호출은 항상 `false` (멱등). 명령형 command 라 startup emit-race 가 없다
/// (프론트가 준비된 뒤 스스로 당겨간다).
///
/// `app_meta`(blocking SQLite)를 읽고 쓰므로 async 워커 풀을 굶기지 않도록
/// `spawn_blocking`으로 옮긴다 (F-RR-06 async-safety).
#[command]
pub async fn take_microphone_upgrade_notice(
    state: tauri::State<'_, AppState>,
) -> Result<bool, IpcError> {
    let storage = state.storage.clone();
    let consent_manager = state.capture.consent_manager.clone();
    let audio_enabled = state.config.audio.enabled;

    tokio::task::spawn_blocking(move || {
        let already_shown = storage.get_meta(MIC_UPGRADE_NOTICE_FLAG).is_some();
        let microphone_granted = consent_manager
            .as_ref()
            .map(|cm| cm.effective_permissions().microphone)
            .unwrap_or(false);
        let show =
            should_show_microphone_upgrade_notice(audio_enabled, microphone_granted, already_shown);
        if show {
            // 표시할 때만 플래그를 기록해 정확히 1회만 노출되도록 한다.
            storage.set_meta(MIC_UPGRADE_NOTICE_FLAG, "true");
        }
        show
    })
    .await
    .map_err(|join_err| {
        IpcError::new(
            "internal.generic",
            format!("take_microphone_upgrade_notice task join failed: {join_err}"),
        )
    })
}

// ---------------------------------------------------------------------------
// 테스트
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::consent::ConsentManager;

    /// set_consent → get_consent 라운드트립 + 감사 로그 기록 검증.
    ///
    /// - `AppState` 없이 bare `ConsentManager` + `SqliteStorage`를 직접 사용한다.
    /// - `apply_set_consent` / `read_consent_snapshot` 헬퍼를 호출한다.
    /// - 결과 스냅샷의 status == Valid, screen_capture == true를 단언한다.
    /// - `entries_by_command_id("system.consent")` 에서 `action_type == "consent_granted"` 항목 존재를 단언한다.
    #[test]
    fn set_then_get_consent_round_trip_and_audit() {
        let dir = tempfile::tempdir().unwrap();
        let cm = std::sync::Arc::new(maekon_core::consent::ConsentManager::new(
            dir.path().join("consent.json"),
        ));
        let storage = std::sync::Arc::new(
            maekon_storage::sqlite::SqliteStorage::open(&dir.path().join("s.db"), 1, None).unwrap(),
        );

        // 동의 부여 + 스냅샷 검증
        let snap = apply_set_consent(
            cm.as_ref(),
            &storage,
            maekon_core::consent::ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
        )
        .expect("apply_set_consent은 쓰기 가능한 임시 디렉터리에서 실패해선 안 된다");
        assert_eq!(snap.status, maekon_core::consent::ConsentStatus::Valid);
        assert!(snap.permissions.screen_capture);

        // read_consent_snapshot 재확인 (독립적 읽기)
        let got = read_consent_snapshot(cm.as_ref());
        assert!(got.permissions.screen_capture);

        // 감사 로그 검증: consent_granted 항목이 1개 이상 존재해야 한다.
        let audit = storage.entries_by_command_id("system.consent", 10);
        assert!(
            audit.iter().any(|e| e.action_type == "consent_granted"),
            "audit_log에 consent_granted 항목이 없음: {audit:?}"
        );
    }

    /// 철회 감사가 철회된(직전) 권한 집합을 기록하는지 검증 (GDPR Art. 7§1 #4684).
    /// 회귀: 이전엔 all-false default를 기록해 "무엇이 철회됐는지" 재구성이 불가했다.
    #[test]
    fn withdraw_consent_audits_the_revoked_permission_set() {
        let dir = tempfile::tempdir().unwrap();
        let cm = std::sync::Arc::new(maekon_core::consent::ConsentManager::new(
            dir.path().join("consent.json"),
        ));
        let storage = std::sync::Arc::new(
            maekon_storage::sqlite::SqliteStorage::open(&dir.path().join("s.db"), 1, None).unwrap(),
        );
        apply_set_consent(
            cm.as_ref(),
            &storage,
            maekon_core::consent::ConsentPermissions {
                screen_capture: true,
                microphone: true,
                ..Default::default()
            },
        )
        .expect("grant");
        let snap = apply_withdraw_consent(cm.as_ref(), &storage).expect("withdraw");

        assert_eq!(snap.status, maekon_core::consent::ConsentStatus::NotGranted);
        assert!(!snap.permissions.screen_capture);
        assert!(!cm.effective_permissions().screen_capture);
        assert!(cm.has_pending_deletion());

        let audit = storage.entries_by_command_id("system.consent", 10);
        let revoke = audit
            .iter()
            .find(|e| e.action_type == "consent_revoked")
            .expect("consent_revoked 감사 항목이 있어야 한다");
        let details: serde_json::Value =
            serde_json::from_str(revoke.details.as_ref().expect("details")).expect("details json");
        // 철회된 권한 집합(prior)이 기록되어야 한다 — all-false 기본값이 아니라.
        assert_eq!(
            details["permissions"]["microphone"],
            serde_json::json!(true),
            "철회 감사는 직전 microphone=true 를 기록해야 한다 (all-false 아님): {details}"
        );
        assert_eq!(
            details["permissions"]["screen_capture"],
            serde_json::json!(true)
        );
    }

    /// #5056: the consent→exporter gate the re-apply helper computes is OFF
    /// after a telemetry-consent revoke, even when config.telemetry.enabled=true.
    ///
    /// The IPC command `reapply_telemetry_gate` resolves managed state (Handle /
    /// ConfigRuntimeState / AppState), which is not constructible headless — but
    /// the load-bearing computation IS the pure
    /// `consent_gated_telemetry_config(&config, &consent_manager)` call. This
    /// test exercises exactly that pure gate through a real `ConsentManager`
    /// grant→revoke lifecycle, proving the helper drives the exporter OFF on
    /// revoke (fail-closed). The IPC wiring itself (`set_consent` /
    /// `withdraw_consent` calling `reapply_telemetry_gate`) is documented and
    /// covered by the manual call-site edits.
    #[test]
    fn reapply_gate_computes_off_after_telemetry_revoke() {
        use crate::telemetry::consent_gated_telemetry_config;
        use maekon_core::config::TelemetryConfig;

        let dir = tempfile::tempdir().unwrap();
        let cm = ConsentManager::new(dir.path().join("consent.json"));

        // User has telemetry config ON.
        let config = TelemetryConfig {
            enabled: true,
            ..Default::default()
        };

        // Grant telemetry consent → gate OPEN (config on + consent on).
        cm.grant_consent(
            ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            RETENTION_DAYS,
        )
        .expect("grant");
        assert!(
            consent_gated_telemetry_config(&config, &cm).enabled,
            "config on + telemetry consent on → exporter ON"
        );

        // Revoke consent → gate CLOSED immediately even though config stays ON.
        cm.revoke_consent().expect("revoke");
        assert!(
            !consent_gated_telemetry_config(&config, &cm).enabled,
            "after revoke the gate computes OFF (fail-closed) — this is what \
             reapply_telemetry_gate pushes to Handle::apply on consent change"
        );
    }

    /// #4686: 마이크 업그레이드 알림 판정 로직 (순수 함수, 진리표).
    #[test]
    fn microphone_upgrade_notice_decision_truth_table() {
        // audio ON + mic 미동의 + 미표시 → 표시한다.
        assert!(should_show_microphone_upgrade_notice(true, false, false));
        // 이미 1회 표시함 → 다시는 표시하지 않는다.
        assert!(!should_show_microphone_upgrade_notice(true, false, true));
        // mic 동의가 이미 있음 → 설명할 것이 없다.
        assert!(!should_show_microphone_upgrade_notice(true, true, false));
        // audio OFF → 마이크를 쓰지 않으므로 관련 없다.
        assert!(!should_show_microphone_upgrade_notice(false, false, false));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // #4801 GDPR Art. 17 테스트
    // ─────────────────────────────────────────────────────────────────────────

    /// 모든 메서드 호출을 추적하는 수동 Mock FrameStoragePort.
    ///
    /// `delete_all_frames` 호출 횟수와 반환할 결과를 제어한다.
    /// mockall 없이 순수 수동 구현 (ADR-001 §5).
    struct MockFrameStorage {
        /// `delete_all_frames` 호출 횟수.
        delete_call_count: std::sync::atomic::AtomicU32,
        /// true이면 `delete_all_frames`가 CoreError::Storage를 반환한다.
        should_fail: bool,
    }

    impl MockFrameStorage {
        /// 성공을 반환하는 Mock.
        fn success() -> Arc<Self> {
            Arc::new(Self {
                delete_call_count: std::sync::atomic::AtomicU32::new(0),
                should_fail: false,
            })
        }

        /// 오류를 반환하는 Mock.
        fn failing() -> Arc<Self> {
            Arc::new(Self {
                delete_call_count: std::sync::atomic::AtomicU32::new(0),
                should_fail: true,
            })
        }

        fn delete_call_count(&self) -> u32 {
            self.delete_call_count
                .load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl maekon_core::ports::frame_storage::FrameStoragePort for MockFrameStorage {
        async fn save_frame(
            &self,
            _ts: chrono::DateTime<chrono::Utc>,
            _data: &[u8],
        ) -> Result<std::path::PathBuf, maekon_core::error::CoreError> {
            unimplemented!("테스트에서 사용되지 않음")
        }

        async fn save_frames_batch(
            &self,
            _frames: Vec<(chrono::DateTime<chrono::Utc>, Vec<u8>)>,
        ) -> Vec<Result<std::path::PathBuf, maekon_core::error::CoreError>> {
            unimplemented!("테스트에서 사용되지 않음")
        }

        async fn load_frame(
            &self,
            _path: &std::path::Path,
        ) -> Result<Vec<u8>, maekon_core::error::CoreError> {
            unimplemented!("테스트에서 사용되지 않음")
        }

        async fn enforce_retention(&self) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("테스트에서 사용되지 않음")
        }

        async fn enforce_storage_limit(&self) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("테스트에서 사용되지 않음")
        }

        async fn delete_all_frames(&self) -> Result<usize, maekon_core::error::CoreError> {
            self.delete_call_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if self.should_fail {
                Err(maekon_core::error::CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: "mock frame delete failure".into(),
                })
            } else {
                Ok(0)
            }
        }
    }

    /// 헬퍼: 임시 디렉터리에 `SqliteStorage`를 열고 일부 사용자 데이터를 삽입한다.
    fn open_storage_with_data() -> (maekon_storage::sqlite::SqliteStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            maekon_storage::sqlite::SqliteStorage::open(&dir.path().join("s.db"), 1, None).unwrap();
        // 이벤트 1건 삽입 — 삭제 후 비어있는지 검증하기 위해.
        // #4928: connection_arc() 는 Arc<GuardedConnection> 를 반환한다 — write_lock funnel 사용.
        let conn = storage.connection_arc();
        conn.write_lock()
            .run::<_, usize, rusqlite::Error>(0, |c| {
                c.execute(
                    "INSERT INTO events (event_id, event_type, timestamp, data) \
                     VALUES ('e1', 'context', '2026-01-01T00:00:00Z', '{}')",
                    [],
                )
            })
            .unwrap();
        (storage, dir)
    }

    /// SQLite 테이블의 행 수를 반환하는 헬퍼.
    fn count_rows(storage: &maekon_storage::sqlite::SqliteStorage, table: &str) -> i64 {
        // 읽기 — read_lock funnel.
        let conn = storage.connection_arc();
        let read = conn.read_lock();
        read.conn()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap_or(0)
    }

    // ── 테스트 1: Phase-1 성공 + Phase-2 성공 ────────────────────────────────

    /// revoke 후 erase_all_local_data는 SQLite를 비우고 delete_all_frames를 호출한다 (#4801).
    #[tokio::test]
    async fn erase_all_local_data_sqlite_empty_and_frames_deleted() {
        let (storage, _dir) = open_storage_with_data();
        let storage = Arc::new(storage);
        let mock_fs = MockFrameStorage::success();

        assert!(
            count_rows(&storage, "events") > 0,
            "이벤트가 삽입되어야 한다"
        );

        erase_all_local_data(
            storage.clone(),
            Some(mock_fs.clone() as Arc<dyn FrameStoragePort>),
        )
        .await
        .expect("erase_all_local_data는 성공해야 한다");

        assert_eq!(
            count_rows(&storage, "events"),
            0,
            "SQLite events 테이블이 비어야 한다"
        );
        assert_eq!(
            mock_fs.delete_call_count(),
            1,
            "delete_all_frames가 정확히 1회 호출되어야 한다"
        );
        // 성공 후 재시도 마커가 없어야 한다.
        assert!(
            storage.get_meta(PENDING_LOCAL_ERASE_KEY).is_none(),
            "성공 후 pending_local_erase 마커가 없어야 한다"
        );
    }

    // ── 테스트 2: Phase-2 실패 → 마커 기록 + Err 반환 (R3/R5) ───────────────

    /// Phase-2(프레임 삭제) 실패 시 pending_local_erase 마커가 기록되고 Err가 반환된다 (#4801 R3/R5).
    #[tokio::test]
    async fn erase_all_local_data_phase2_failure_sets_marker_and_returns_err() {
        let (storage, _dir) = open_storage_with_data();
        let storage = Arc::new(storage);
        let mock_fs = MockFrameStorage::failing();

        let result = erase_all_local_data(
            storage.clone(),
            Some(mock_fs.clone() as Arc<dyn FrameStoragePort>),
        )
        .await;

        // R5: 프레임 삭제 실패 시 Err를 반환해야 한다 (Ok로 은폐 금지).
        let ipc_err = result.unwrap_err();
        assert!(
            ipc_err.code.contains("storage"),
            "Phase-2 실패 시 storage IpcError를 반환해야 한다 (R5)"
        );

        // R3: 재시도 마커가 기록되어야 한다.
        assert_eq!(
            storage.get_meta(PENDING_LOCAL_ERASE_KEY),
            Some("1".to_string()),
            "Phase-2 실패 후 pending_local_erase=1 마커가 기록되어야 한다 (R3)"
        );

        // Phase-1(SQLite)은 이미 완료되어 events 테이블이 비어있어야 한다.
        assert_eq!(
            count_rows(&storage, "events"),
            0,
            "Phase-1(SQLite 삭제)은 성공했으므로 events 테이블이 비어야 한다"
        );
    }

    // ── 테스트 3: 마커가 있을 때 retry_pending_local_erase가 재시도하고 마커를 지운다 ──

    /// 마커가 있을 때 retry_pending_local_erase는 삭제를 재시도하고 성공 시 마커를 지운다 (#4801 R2).
    #[tokio::test]
    async fn retry_pending_local_erase_reruns_and_clears_marker_on_success() {
        let (storage, _dir) = open_storage_with_data();
        let storage = Arc::new(storage);

        // 재시도 마커를 수동으로 기록한다 (이전 기동에서 Phase-2 실패 시뮬레이션).
        storage
            .set_meta_checked(PENDING_LOCAL_ERASE_KEY, "1")
            .unwrap();

        let mock_fs = MockFrameStorage::success();
        retry_pending_local_erase(
            storage.clone(),
            Some(mock_fs.clone() as Arc<dyn FrameStoragePort>),
        )
        .await;

        // 재시도 성공 후 마커가 삭제되어야 한다.
        assert!(
            storage.get_meta(PENDING_LOCAL_ERASE_KEY).is_none(),
            "재시도 성공 후 pending_local_erase 마커가 삭제되어야 한다"
        );

        // delete_all_frames가 재시도 중 호출되어야 한다.
        assert_eq!(
            mock_fs.delete_call_count(),
            1,
            "retry 경로에서 delete_all_frames가 호출되어야 한다"
        );
    }

    // ── 테스트 4: 마커가 없을 때 retry_pending_local_erase는 no-op ────────────

    /// 마커가 없으면 retry_pending_local_erase는 아무것도 하지 않는다 (#4801 — 정상 기동 경로).
    #[tokio::test]
    async fn retry_pending_local_erase_noop_when_no_marker() {
        let (storage, _dir) = open_storage_with_data();
        let storage = Arc::new(storage);

        // 마커 없이 retry 실행.
        let mock_fs = MockFrameStorage::success();
        retry_pending_local_erase(
            storage.clone(),
            Some(mock_fs.clone() as Arc<dyn FrameStoragePort>),
        )
        .await;

        // delete_all_frames가 호출되지 않아야 한다.
        assert_eq!(
            mock_fs.delete_call_count(),
            0,
            "마커가 없으면 delete_all_frames가 호출되지 않아야 한다"
        );

        // 이벤트 데이터는 그대로 있어야 한다.
        assert!(
            count_rows(&storage, "events") > 0,
            "마커 없이 retry 시 데이터가 삭제되어선 안 된다"
        );
    }

    // ── 테스트 5: frame_storage가 None이면 Phase-2를 건너뛴다 ────────────────

    /// frame_storage가 None이면 Phase-1만 실행하고 Ok를 반환한다.
    #[tokio::test]
    async fn erase_all_local_data_no_frame_storage_erases_sqlite_only() {
        let (storage, _dir) = open_storage_with_data();
        let storage = Arc::new(storage);

        assert!(count_rows(&storage, "events") > 0);

        // frame_storage = None — Phase-2 생략.
        erase_all_local_data(storage.clone(), None)
            .await
            .expect("frame_storage가 None이어도 Ok를 반환해야 한다");

        assert_eq!(
            count_rows(&storage, "events"),
            0,
            "frame_storage=None이어도 SQLite는 삭제되어야 한다"
        );
    }
}
