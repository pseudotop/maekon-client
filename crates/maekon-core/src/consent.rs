use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::CoreError;

pub const CURRENT_POLICY_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsentPermissions {
    // --- Tier 1 ---
    #[serde(default)]
    pub screen_capture: bool,
    #[serde(default)]
    pub ocr_processing: bool,
    #[serde(default)]
    pub telemetry: bool,
    #[serde(default)]
    pub process_monitoring: bool,
    #[serde(default)]
    pub input_activity: bool,

    // --- Tier 2 ---
    #[serde(default)]
    pub window_title_collection: bool,
    #[serde(default)]
    pub app_usage_analytics: bool,

    // --- Tier 3 ---
    #[serde(default)]
    pub clipboard_monitoring: bool,
    #[serde(default)]
    pub file_access_monitoring: bool,

    // --- Tier 4: Tiered Memory ---
    #[serde(default)]
    pub activity_pattern_learning: bool,

    // --- Tier 5: Cross-Device Sync ---
    /// Permits cross-device synchronization of activity data.
    /// GDPR Article 6 -- processing requires explicit consent for data
    /// transfer between devices, even when both are owned by the same user.
    #[serde(default)]
    pub cross_device_sync: bool,

    // --- Tier 6: Text Intelligence ---
    /// Permits extraction of full text content from focused UI elements.
    /// Required only when pii_extraction_level is set to Off.
    /// GDPR Article 6 -- explicit consent for processing text content
    /// that may contain personal data.
    #[serde(default)]
    pub full_text_extraction: bool,

    // --- Tier 7: Memory-Graph Enrichment ---
    /// Permits feeding durable, activity-derived memory-graph claim text to a
    /// LOCAL LLM for relation/contradiction inference (ADR-023 Phase-2 D1/D2).
    /// GDPR Article 6 -- explicit consent. Default false (fail-closed); this is a
    /// dedicated permission, NOT borrowed from `full_text_extraction` (which gates
    /// the active-window external-LLM path) or `activity_pattern_learning`.
    #[serde(default)]
    pub memory_graph_enrichment: bool,

    // --- Tier 8: Audio/Voice ---
    /// Permits microphone capture (continuous voice-activity listening + push-to-talk)
    /// and the resulting speech-to-text. Higher sensitivity than screen: captures
    /// audio + transcripts, and -- if `audio.stt_provider` is Cloud with an API key --
    /// sends raw audio off-device, unfiltered, to a third-party endpoint (#4568).
    /// GDPR Article 6 -- explicit consent. Default false (fail-closed); this is a
    /// dedicated permission, NOT borrowed from `screen_capture` (granting screen
    /// consent must never silently authorize the mic).
    #[serde(default)]
    pub microphone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentRecord {
    pub consent_id: String,
    pub version: String,
    pub granted_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Timestamp recorded when the user revokes consent (GDPR Article 17 audit trail).
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
    /// Set to true after revocation to signal that queued data must be purged
    /// before the next upload cycle (GDPR Article 17 — right to erasure).
    #[serde(default)]
    pub data_deletion_requested: bool,
    /// #5165: a collision-proof per-revoke nonce (a ULID), minted at revoke time so
    /// the per-erasure id never rests on wall-clock distinctness. `None` for a
    /// granted record and for any erasure revoked before this field existed — in the
    /// latter case `pending_erasure_id` falls back to the `revoked_at` timestamp so
    /// an in-flight pre-upgrade erasure keeps its restart identity (no re-announce).
    #[serde(default)]
    pub erasure_nonce: Option<String>,
    pub permissions: ConsentPermissions,
    pub data_retention_days: u32,
}

// F-RC-C37-04: explicit PascalCase wire contract. Serde default would also produce
// PascalCase for these variants, but the attribute makes the contract undeniable.
// Existing consent JSON files use PascalCase (written by ConsentManager::save_to_file)
// so this is fully backward-compatible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ConsentStatus {
    NotGranted,
    Valid,
    Expired,
    UpdateRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDataExport {
    pub exported_at: DateTime<Utc>,
    pub consent: Option<ConsentRecord>,
    pub settings: serde_json::Value,
    pub event_count: u64,
    pub frame_count: u64,
    pub export_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletionResult {
    pub deleted_at: DateTime<Utc>,
    pub events_deleted: u64,
    pub frames_deleted: u64,
    pub metrics_deleted: u64,
    pub settings_reset: bool,
    pub consent_revoked: bool,
}

// ConsentManager

/// 내부 가변 상태. `ConsentManager`가 공유 불변 `Arc`로 배포되더라도 쓰기가
/// 가능하도록 `RwLock`으로 감싼다 (writers는 `&self`).
struct ConsentState {
    current_consent: Option<ConsentRecord>,
    /// revoke_consent() 호출 후 데이터 소거가 완료되기 전까지 true를 유지한다.
    /// current_consent = None 이후에도 GDPR Article 17 신호가 소실되지 않도록
    /// 별도 in-memory 플래그로 관리한다.
    pending_deletion: bool,
    /// #5156: 현재 대기 중인 Art. 17 erasure 의 시각(= 영속화된
    /// `ConsentRecord.revoked_at` 미러). revoke 마다 변하는 restart-stable,
    /// per-erasure-distinct 식별자를 제공한다. load 시 영속 record 에서 복구되어
    /// REMOTE 전파 신호가 restart 를 견딘다. `pending_deletion` 과 함께 clear 되며,
    /// grant 는 건드리지 않는다(#4630-b). LOCAL 쓰기 게이트(`deletion_flag`)와는
    /// 별개 — restart 시 deletion_flag 는 false 유지(로컬 소거는 revoke 세션에서 완료).
    pending_erasure_at: Option<DateTime<Utc>>,
    /// #5165: collision-proof per-revoke nonce (mirror of `ConsentRecord.erasure_nonce`),
    /// the preferred per-erasure id. Set/cleared in lockstep with `pending_erasure_at`;
    /// recovered on load. `None` for a pre-upgrade in-flight erasure → `pending_erasure_id`
    /// falls back to the `revoked_at` timestamp.
    pending_erasure_nonce: Option<String>,
}

pub struct ConsentManager {
    storage_path: PathBuf,
    state: parking_lot::RwLock<ConsentState>,
    /// #4928: consent-revoke erasure 차단 chokepoint 의 LIVE 신호.
    ///
    /// `pending_deletion` 을 미러링한다 — revoke → `true`, grant/clear → `false`.
    /// composition root 에서 이 동일 `Arc` 를 `SqliteStorage::set_deletion_flag` 와
    /// `FrameFileStorage::set_deletion_flag` 에 install 하면, revoke 이후 진입한
    /// in-flight writer 가 funnel(`write_lock`/frame 배리어)에서 자동으로 스킵된다.
    /// consent 게이트가 steady-state 보호이고, 이 flag 는 erase-window in-flight
    /// race 의 backstop 이다 (스펙 §2/§3bis).
    deletion_flag: Arc<AtomicBool>,
    /// #4928 round-3 (FIX B): grant_consent-during-erase TOCTOU 차단 신호.
    ///
    /// `deletion_flag` 는 `grant_consent` 가 `false` 로 되돌릴 수 있어, erase 윈도우
    /// (Phase-1 커밋 후 ~ Phase-2 진행 중) 안에 재동의가 끼어들면 in-flight writer 가
    /// flag clear 를 보고 wipe 이후 잔존 행을 쓸 수 있다. `erasing` 은
    /// `erase_all_local_data` 가 RAII 로 set/clear 하며 `grant_consent`/`clear_pending_deletion`
    /// 은 절대 건드리지 못한다. composition root 가 이 동일 `Arc` 를
    /// `SqliteStorage::set_erasing` / `FrameFileStorage::set_erasing` 에 install 한다.
    /// write skip 술어는 `deletion_flag || erasing` 이다.
    erasing: Arc<AtomicBool>,
}

impl ConsentManager {
    pub fn new(storage_path: PathBuf) -> Self {
        // #5156: recover a pending erasure's identity (`revoked_at`) from a
        // revoked-but-not-erased record so the per-erasure id (`pending_erasure_id`)
        // is available across restart for the sync engine's gate.
        //
        // ⚠️ DURABILITY DEFERRED (#5156 stage-1 guard): we do NOT re-arm
        // `pending_deletion` on restart here. Re-arming it makes `has_pending_deletion()`
        // true after restart, which the (current) sync gate turns into a re-announced
        // DeletionEvent — and `handle_deletion_event` is an UNBOUNDED
        // `DELETE WHERE origin_device_id` (sync_merger.rs), so a re-announce after a
        // revoke→re-grant→sync-new-data sequence would DELETE the new data on peers
        // (data loss). Restart durability is re-enabled SAFELY in stage 2 via a
        // *persisted, sender-side fire-once* gate keyed on `pending_erasure_id` (the
        // sender propagates each distinct erasure exactly once, never re-announcing).
        // Public sync/erasure semantics: docs/guides/sync-conflict-resolution.md.
        let (current_consent, pending_erasure_at, pending_erasure_nonce) =
            Self::load_from_file(&storage_path);
        let pending_deletion = false;
        Self {
            storage_path,
            state: parking_lot::RwLock::new(ConsentState {
                current_consent,
                pending_deletion,
                pending_erasure_at,
                pending_erasure_nonce,
            }),
            // #4928: erasure 차단 flag 는 false(쓰기 허용)로 초기화한다. restart 시
            // pending erasure 가 있어도 LOCAL 소거는 revoke 세션에서 이미 완료됐으므로
            // 로컬 쓰기를 다시 막지 않는다(REMOTE 전파만 미완일 수 있음).
            deletion_flag: Arc::new(AtomicBool::new(false)),
            // #4928 round-3: erase-window 차단 신호도 false 로 초기화한다.
            erasing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// #4928: erasure 차단 chokepoint 의 공유 `deletion_flag` 를 반환한다.
    ///
    /// composition root 가 이 `Arc` 를 `SqliteStorage`/`FrameFileStorage` 에
    /// install 해 동일 신호를 공유한다 (ptr-eq). revoke 시 `true`, grant/clear 시
    /// `false` 로 미러링된다.
    pub fn deletion_flag(&self) -> Arc<AtomicBool> {
        self.deletion_flag.clone()
    }

    /// #4928 round-3 (FIX B): erase-window 차단 신호 `erasing` 을 반환한다.
    ///
    /// composition root 가 이 `Arc` 를 `SqliteStorage::set_erasing` /
    /// `FrameFileStorage::set_erasing` 에 install 해 동일 신호를 공유한다 (ptr-eq).
    /// `erase_all_local_data` 만 RAII 로 set/clear 하며 `grant_consent` 는 clear 하지 못한다.
    pub fn erasing(&self) -> Arc<AtomicBool> {
        self.erasing.clone()
    }

    /// 이미 획득된 가드의 상태에 대해 동의 유효성을 평가한다 (락 재진입 없음).
    /// `check_consent`와 `effective_permissions` 양쪽이 공유하는 단일 유효성 판단
    /// 로직이다 — 한 가드 내에서 평가하므로 두 메서드 간 TOCTOU 창이 없다.
    fn check_consent_locked(st: &ConsentState) -> ConsentStatus {
        match &st.current_consent {
            None => ConsentStatus::NotGranted,
            Some(record) => {
                if let Some(expires) = record.expires_at {
                    if Utc::now() > expires {
                        return ConsentStatus::Expired;
                    }
                }
                if record.version != CURRENT_POLICY_VERSION {
                    return ConsentStatus::UpdateRequired;
                }
                ConsentStatus::Valid
            }
        }
    }

    pub fn check_consent(&self) -> ConsentStatus {
        Self::check_consent_locked(&self.state.read())
    }

    /// 현재 동의 레코드의 소유 스냅샷 (이전 시그니처는 `Option<&ConsentRecord>`).
    /// 읽기 락 하에서 clone 하여 반환한다.
    pub fn current_consent(&self) -> Option<ConsentRecord> {
        self.state.read().current_consent.clone()
    }

    /// 동의가 현재 Valid일 때만 권한을 반환하고, 그 외에는 모두 false를 반환한다.
    /// 모든 게이트에서 이 메서드를 사용해 Expired/UpdateRequired/부재 상태가
    /// fail-closed 되도록 한다.
    ///
    /// 단일 읽기 가드 하에서 유효성 검사와 권한 추출을 수행하므로 두 연산 사이에
    /// torn-state 창이 없다.
    pub fn effective_permissions(&self) -> ConsentPermissions {
        let st = self.state.read(); // 단일 가드 — 유효성 검사와 권한 추출 모두 이 가드 내에서
        if Self::check_consent_locked(&st) == ConsentStatus::Valid {
            st.current_consent
                .as_ref()
                .map(|r| r.permissions.clone())
                .unwrap_or_default()
        } else {
            ConsentPermissions::default()
        }
    }

    /// 단일 read 가드로 상태와 (원본) 권한 집합을 원자적으로 읽는다 (UI 스냅샷용, TOCTOU 제거).
    ///
    /// `check_consent()` + `current_consent()`를 별도 호출하면 두 read 락 사이에
    /// grant/revoke가 끼어들어 status와 permissions가 불일치하는 스냅샷이 나올 수
    /// 있다. 이 메서드는 한 가드 안에서 둘 다 읽어 그 창을 제거한다.
    ///
    /// `effective_permissions`와 달리 비-Valid 상태에서도 권한을 0으로 만들지 않고
    /// **원본 부여 권한**(`current_consent.permissions`)을 그대로 반환한다 — UI는
    /// 상태(예: Expired)와 함께 "무엇이 부여되었는지"를 보여줘야 하기 때문이다.
    /// 게이트 판정에는 절대 이 권한을 쓰면 안 된다 (fail-closed는 `effective_permissions`).
    pub fn status_and_permissions(&self) -> (ConsentStatus, ConsentPermissions) {
        let st = self.state.read(); // 단일 가드 — 상태 판정과 원본 권한 추출을 함께
        let status = Self::check_consent_locked(&st);
        let permissions = st
            .current_consent
            .as_ref()
            .map(|r| r.permissions.clone())
            .unwrap_or_default();
        (status, permissions)
    }

    pub fn grant_consent(
        &self,
        permissions: ConsentPermissions,
        data_retention_days: u32,
    ) -> Result<(), CoreError> {
        // ADR-022: client-generated IDs follow prefix+ULID convention (generate_id).
        // consent_id is a String field used locally for audit trail; not validated
        // against UUID format anywhere in the codebase (grep confirmed 2026-05-28).
        let record = ConsentRecord {
            consent_id: crate::id_generation::generate_id("consent"),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now(),
            expires_at: None,
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions,
            data_retention_days,
        };

        // 파일 저장은 불변 storage_path만 읽으므로 락 획득 전에 수행한다.
        self.save_to_file(&record)?;
        // #4630 (decision b — erasure is irrevocable): we intentionally do NOT clear
        // `pending_deletion` here. If the user revoked (requesting an Art. 17 erasure)
        // and re-grants before that erasure has propagated, the prior erasure still
        // stands — it still fires the cross-device DeletionEvent on the next sync — and
        // this grant only opens a fresh local collection window. Retracting a pending
        // erasure here is the rejected option (a) in #4630.
        self.state.write().current_consent = Some(record);
        // #4928: 재동의는 LOCAL 수집 윈도우를 새로 연다 — erasure 차단 flag 를 clear
        // 하여 funnel 쓰기를 재개한다. (REMOTE 전파 신호인 `pending_deletion` 은
        // #4630(b) 에 따라 grant 가 건드리지 않는다 — 둘은 별개의 신호다: flag 는
        // in-flight 로컬 쓰기 게이트, pending_deletion 은 미전파 erasure 마커.)
        self.deletion_flag.store(false, Ordering::Release);
        Ok(())
    }

    /// Revokes user consent (GDPR Article 7 §3).
    ///
    /// Sets `data_deletion_requested = true` and `revoked_at` on the record,
    /// then atomically writes the updated record to disk (write to .tmp, rename)
    /// so that a crash between write and rename never leaves a partially-written
    /// or deleted file.  `load_from_file()` treats `data_deletion_requested =
    /// true` as revoked, so the file's presence does not re-activate consent.
    pub fn revoke_consent(&self) -> Result<(), CoreError> {
        // read-mutate-persist-clear 전 구간을 단일 write 가드로 보호한다.
        // (중간에 가드를 놓았다 다시 잡으면 readers에게 torn state 윈도우가 생긴다.)
        // #5156: capture the revocation instant once so the persisted `revoked_at`
        // and the in-memory per-erasure id are the SAME value (restart recovery
        // must round-trip to an identical id).
        let now = Utc::now();
        // #5165: a collision-proof per-revoke nonce (ULID), independent of wall-clock,
        // becomes the per-erasure id so two same-instant or clock-rewound revokes never
        // collide. Persisted on the record so it round-trips across restart.
        let nonce = crate::id_generation::generate_id("erasure");
        let mut st = self.state.write();
        let mut revoked_active_consent = false;
        if let Some(record) = st.current_consent.as_mut() {
            record.revoked_at = Some(now);
            record.data_deletion_requested = true;
            record.erasure_nonce = Some(nonce.clone());
            revoked_active_consent = true;
        }
        // Clone to release the mutable borrow before writing.
        if let Some(record) = st.current_consent.clone() {
            // Atomic write: serialize to a .tmp file, then rename into place.
            // This eliminates the TOCTOU window that existed when we saved and
            // then deleted the file in two separate syscalls.
            let tmp_path = self.consent_file_path().with_extension("tmp");
            if let Some(parent) = tmp_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let json = serde_json::to_string_pretty(&record)?;
            std::fs::write(&tmp_path, &json)?;
            std::fs::rename(&tmp_path, self.consent_file_path())?;
        }
        st.current_consent = None;
        // current_consent를 None으로 설정한 뒤에도 소거 요청 신호가 소실되지
        // 않도록 in-memory 플래그를 true로 유지한다 (GDPR Article 17).
        st.pending_deletion = true;
        // #5156: advance the per-erasure id ONLY when an ACTIVE consent was revoked
        // (a genuinely new erasure of newly-collected data). A repeat revoke with no
        // active consent collects nothing new → keep the existing pending id (and the
        // already-persisted revoked_at), so it is not mistaken for a distinct erasure.
        if revoked_active_consent {
            st.pending_erasure_at = Some(now);
            st.pending_erasure_nonce = Some(nonce);
        }
        // #4928: erase 직전에 erasure 차단 flag 를 set 한다 (erase 전에 set 되어야
        // in-flight writer 가 wipe 와 race 하지 않는다). 이 시점 이후 funnel 에
        // 진입한 쓰기는 모두 no-op 으로 스킵된다.
        self.deletion_flag.store(true, Ordering::Release);
        Ok(())
    }

    /// Returns true when consent was previously revoked and local data is
    /// pending erasure (GDPR Article 17).  Callers should purge stored events,
    /// frames, and metrics before the next server sync when this returns true.
    ///
    /// `pending_deletion` 플래그는 `revoke_consent()` 이후 `current_consent`가
    /// None으로 바뀌더라도 true를 유지한다. 데이터 소거 완료 후에는
    /// `clear_pending_deletion()`을 호출해 플래그를 초기화해야 한다.
    pub fn has_pending_deletion(&self) -> bool {
        // in-memory 플래그 우선 확인 — revoke 이후 current_consent가 None이어도
        // 소거 신호가 보존된다.
        let st = self.state.read();
        st.pending_deletion
            // Defensive/legacy branch: since #5156, `load_from_file` never yields a
            // `current_consent` with `data_deletion_requested == true` (a revoked
            // record returns `(None, Some(revoked_at))` and drives `pending_deletion`
            // instead), and `grant`/`revoke` keep it false/None — so this is now
            // effectively dead but kept as a harmless guard.
            || st
                .current_consent
                .as_ref()
                .map(|r| r.data_deletion_requested)
                .unwrap_or(false)
    }

    /// #5156/#5165: a restart-stable, per-erasure-distinct id for the currently
    /// pending Art. 17 erasure, or `None` when none is pending. Prefers the
    /// collision-proof per-revoke nonce (#5165); falls back to the `revoked_at`
    /// instant (rfc3339) for an in-flight erasure revoked before the nonce existed,
    /// so its restart identity / fire-once-gate match stays intact across the upgrade.
    /// Changes per revoke (a new erasure) and survives restart (recovered from the
    /// persisted revoked record in `new`). Used by the sync engine to re-propagate a
    /// new erasure + dedup a restart re-fire of the same one.
    pub fn pending_erasure_id(&self) -> Option<String> {
        let st = self.state.read();
        st.pending_erasure_nonce
            .clone()
            .or_else(|| st.pending_erasure_at.map(|at| at.to_rfc3339()))
    }

    /// 데이터 소거 완료 후 호출한다. GDPR Article 17 소거 신호를 초기화한다.
    ///
    /// 이 메서드는 실제 데이터 소거가 완료된 직후에만 호출해야 한다.
    /// 소거 전에 호출하면 삭제 요청이 누락된다.
    pub fn clear_pending_deletion(&self) {
        {
            let mut st = self.state.write();
            st.pending_deletion = false;
            // #5156/#5165: drop the per-erasure id (timestamp + nonce) too, so a
            // completed erasure is not re-propagated/re-audited as if still pending.
            st.pending_erasure_at = None;
            st.pending_erasure_nonce = None;
        }
        // #4928: 소거 완료(또는 원격 전파 완료) 후 차단 flag 를 clear 하여 이후
        // 쓰기를 재개한다. erase 가 끝났으므로 in-flight race 윈도우도 닫혔다.
        self.deletion_flag.store(false, Ordering::Release);
    }

    pub fn is_permitted(&self, check: impl Fn(&ConsentPermissions) -> bool) -> bool {
        self.state
            .read()
            .current_consent
            .as_ref()
            .map(|r| check(&r.permissions))
            .unwrap_or(false)
    }

    /// Returns the canonical path of the consent file (same as `storage_path`).
    fn consent_file_path(&self) -> PathBuf {
        self.storage_path.clone()
    }

    /// Loads the persisted consent state, returning `(active_consent,
    /// pending_erasure_at, pending_erasure_nonce)`. A revoked-but-not-yet-erased
    /// record yields `(None, Some(revoked_at), record.erasure_nonce)`: no active
    /// consent, but the per-erasure id (the #5165 nonce, or the `revoked_at`
    /// timestamp for a pre-upgrade record without a nonce) + the REMOTE propagation
    /// signal survive restart (#5156). A clean granted record yields
    /// `(Some(record), None, None)`.
    fn load_from_file(
        path: &PathBuf,
    ) -> (Option<ConsentRecord>, Option<DateTime<Utc>>, Option<String>) {
        let Ok(data) = std::fs::read_to_string(path) else {
            return (None, None, None);
        };
        let Ok(record) = serde_json::from_str::<ConsentRecord>(&data) else {
            return (None, None, None);
        };
        // データ削除要求が設定されている場合は同意なしとして扱う。
        // Treat a revoked-but-not-yet-erased record as absent consent so that a
        // process restart after revoke_consent() does not re-activate consent —
        // but recover `revoked_at` + the erasure nonce so the pending erasure id is
        // not lost (#5156/#5165).
        if record.data_deletion_requested {
            return (None, record.revoked_at, record.erasure_nonce);
        }
        (Some(record), None, None)
    }

    fn save_to_file(&self, record: &ConsentRecord) -> Result<(), CoreError> {
        // Atomic write: serialize to a .tmp file, then rename into place — a crash
        // mid-write cannot leave a truncated consent.json that load_from_file would
        // reject (silently dropping a consent the user believes they granted).
        // Mirrors revoke_consent's tmp+rename.
        let tmp_path = self.storage_path.with_extension("tmp");
        if let Some(parent) = tmp_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(record)?;
        std::fs::write(&tmp_path, &json)?;
        std::fs::rename(&tmp_path, &self.storage_path)?;
        Ok(())
    }
}

/// ADR-026 PR-1: `ConsentManager` implements the object-safe
/// `ConsentManagerPort` by delegating each method to its inherent counterpart.
///
/// Purely additive — the existing concrete `Arc<ConsentManager>` consumers are
/// untouched; this only opens the `Arc<dyn ConsentManagerPort>` DI path for
/// future migration (ADR-001 §3). `is_permitted` is deliberately absent from
/// the port (it is not dyn-compatible, `E0038`) and stays inherent above.
impl crate::ports::consent_manager::ConsentManagerPort for ConsentManager {
    fn check_consent(&self) -> ConsentStatus {
        ConsentManager::check_consent(self)
    }

    fn current_consent(&self) -> Option<ConsentRecord> {
        ConsentManager::current_consent(self)
    }

    fn effective_permissions(&self) -> ConsentPermissions {
        ConsentManager::effective_permissions(self)
    }

    fn status_and_permissions(&self) -> (ConsentStatus, ConsentPermissions) {
        ConsentManager::status_and_permissions(self)
    }

    fn grant_consent(
        &self,
        permissions: ConsentPermissions,
        data_retention_days: u32,
    ) -> Result<(), CoreError> {
        ConsentManager::grant_consent(self, permissions, data_retention_days)
    }

    fn revoke_consent(&self) -> Result<(), CoreError> {
        ConsentManager::revoke_consent(self)
    }

    fn has_pending_deletion(&self) -> bool {
        ConsentManager::has_pending_deletion(self)
    }

    fn pending_erasure_id(&self) -> Option<String> {
        ConsentManager::pending_erasure_id(self)
    }

    fn clear_pending_deletion(&self) {
        ConsentManager::clear_pending_deletion(self)
    }

    fn deletion_flag(&self) -> Arc<AtomicBool> {
        ConsentManager::deletion_flag(self)
    }

    fn erasing(&self) -> Arc<AtomicBool> {
        ConsentManager::erasing(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #4684: `save_to_file` (the grant persistence path) is atomic (tmp+rename),
    /// matching `revoke_consent` — after a grant the consent file exists, no `.tmp`
    /// leftover remains, and the record reloads as Valid. Regression: the prior
    /// single-call `std::fs::write` could leave a truncated file on a mid-write crash,
    /// which `load_from_file` rejects → silently dropping a consent the user granted.
    #[test]
    fn grant_consent_writes_atomically_no_tmp_leftover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path.clone());
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .expect("grant_consent should succeed in a writable temp dir");
        assert!(
            path.exists(),
            "consent.json must be written by save_to_file"
        );
        assert!(
            !path.with_extension("tmp").exists(),
            ".tmp must be renamed into place (atomic write), not left behind"
        );
        let reloaded = ConsentManager::new(path);
        assert_eq!(reloaded.check_consent(), ConsentStatus::Valid);
        assert!(reloaded.effective_permissions().screen_capture);
    }

    /// F-RC-C37-04: pin exact wire strings for ConsentStatus so any change to
    /// rename_all or variant names is caught immediately.
    #[test]
    fn consent_status_wire_strings_pinned() {
        let cases = [
            (ConsentStatus::NotGranted, "\"NotGranted\""),
            (ConsentStatus::Valid, "\"Valid\""),
            (ConsentStatus::Expired, "\"Expired\""),
            (ConsentStatus::UpdateRequired, "\"UpdateRequired\""),
        ];
        for (status, expected_json) in &cases {
            let json = serde_json::to_string(status).expect("serialize");
            assert_eq!(
                json, *expected_json,
                "ConsentStatus wire string mismatch for {:?}",
                status
            );
            let deser: ConsentStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(
                deser, *status,
                "ConsentStatus round-trip failed for {:?}",
                status
            );
        }
    }

    #[test]
    fn consent_permissions_default_all_false() {
        let perms = ConsentPermissions::default();
        assert!(!perms.screen_capture);
        assert!(!perms.telemetry);
        assert!(!perms.clipboard_monitoring);
    }

    #[test]
    fn consent_record_serde_roundtrip() {
        let record = ConsentRecord {
            consent_id: "test-001".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now(),
            expires_at: None,
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };

        let json = serde_json::to_string(&record).unwrap();
        let deserialized: ConsentRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.consent_id, "test-001");
        assert_eq!(deserialized.data_retention_days, 30);
        assert!(deserialized.revoked_at.is_none());
        assert!(!deserialized.data_deletion_requested);
    }

    #[test]
    fn consent_record_serde_legacy_compat() {
        // Records written before revoked_at / data_deletion_requested were added
        // must still deserialize correctly (both fields have #[serde(default)]).
        let legacy_json = r#"{
            "consent_id": "legacy-001",
            "version": "1.0.0",
            "granted_at": "2025-01-01T00:00:00Z",
            "expires_at": null,
            "permissions": {},
            "data_retention_days": 30
        }"#;
        let record: ConsentRecord = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(record.consent_id, "legacy-001");
        assert!(record.revoked_at.is_none());
        assert!(!record.data_deletion_requested);
    }

    #[test]
    fn consent_status_not_granted_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        assert_eq!(manager.check_consent(), ConsentStatus::NotGranted);
    }

    #[test]
    fn consent_grant_and_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        let perms = ConsentPermissions {
            screen_capture: true,
            ..Default::default()
        };
        manager.grant_consent(perms, 30).unwrap();

        assert_eq!(manager.check_consent(), ConsentStatus::Valid);
        assert!(manager.is_permitted(|p| p.screen_capture));
        assert!(!manager.is_permitted(|p| p.clipboard_monitoring));
    }

    #[test]
    fn consent_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        let perms = ConsentPermissions::default();
        manager.grant_consent(perms, 30).unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);

        manager.revoke_consent().unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::NotGranted);
    }

    #[test]
    fn consent_expired() {
        let dir = tempfile::tempdir().unwrap();

        let record = ConsentRecord {
            consent_id: "expired-001".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(365),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };
        // grant_consent can't synthesize Expired (it hardcodes expires_at: None), so
        // craft the record onto disk and reconstruct the manager from it.
        let path = dir.path().join("consent_expired.json");
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();
        let manager = ConsentManager::new(path);
        assert_eq!(manager.check_consent(), ConsentStatus::Expired);
    }

    #[test]
    fn consent_update_required_on_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();

        let record = ConsentRecord {
            consent_id: "old-001".to_string(),
            version: "0.9.0".to_string(), // previous version
            granted_at: Utc::now(),
            expires_at: None,
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };
        // grant_consent can't synthesize UpdateRequired (it hardcodes the current
        // version), so craft the record onto disk and reconstruct the manager.
        let path = dir.path().join("consent_version_mismatch.json");
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();
        let manager = ConsentManager::new(path);
        assert_eq!(manager.check_consent(), ConsentStatus::UpdateRequired);
    }

    /// #4631: the sub-tier consent reads (ocr / full-text / cross-device / memory-graph)
    /// now flow through `effective_permissions()`, so a non-Valid (Expired) consent denies
    /// them even though the raw record still carries the bits. Locks the defense-in-depth
    /// Valid-gate the 8 migrated call sites rely on (raw `is_permitted` would have permitted).
    #[test]
    fn effective_permissions_valid_gates_sub_tier_fields_when_expired() {
        let dir = tempfile::tempdir().unwrap();
        let record = ConsentRecord {
            consent_id: "expired-subtier".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(365),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                ocr_processing: true,
                cross_device_sync: true,
                full_text_extraction: true,
                memory_graph_enrichment: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        let path = dir.path().join("consent_expired_subtier.json");
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();
        let manager = ConsentManager::new(path);
        // The manager is Expired, and the RAW (non-gated) read still sees the granted bit…
        assert_eq!(manager.check_consent(), ConsentStatus::Expired);
        assert!(
            manager.is_permitted(|p| p.ocr_processing),
            "raw is_permitted still reflects the on-disk bit (this is why the call-site swap matters)"
        );
        // …but the Valid-gated effective_permissions denies every migrated sub-tier field.
        let eff = manager.effective_permissions();
        assert!(!eff.ocr_processing, "expired consent must not permit OCR");
        assert!(
            !eff.cross_device_sync,
            "expired consent must not permit cross-device sync"
        );
        assert!(
            !eff.full_text_extraction,
            "expired consent must not permit full-text extraction"
        );
        assert!(
            !eff.memory_graph_enrichment,
            "expired consent must not permit memory-graph enrichment"
        );
    }

    #[test]
    fn has_pending_deletion_false_before_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert!(!manager.has_pending_deletion());
    }

    #[test]
    fn consent_revoke_records_audit_trail() {
        // 동의 철회 후 has_pending_deletion()은 true를 반환해야 한다
        // (GDPR Article 17 소거 신호가 current_consent = None 이후에도 보존되는지 검증).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);

        manager.revoke_consent().unwrap();
        // 철회 후: 활성 동의 없음
        assert_eq!(manager.check_consent(), ConsentStatus::NotGranted);
        // pending_deletion 플래그는 revoke 이후 true를 유지해야 한다
        assert!(manager.has_pending_deletion());
    }

    #[test]
    fn has_pending_deletion_true_after_revoke() {
        // revoke_consent() → has_pending_deletion() == true →
        // clear_pending_deletion() → has_pending_deletion() == false 전체 라이프사이클 검증.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        // 동의 부여
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert!(
            !manager.has_pending_deletion(),
            "동의 부여 직후에는 소거 대기가 없어야 한다"
        );

        // 동의 철회
        manager.revoke_consent().unwrap();
        assert!(
            manager.has_pending_deletion(),
            "revoke_consent() 이후 has_pending_deletion()은 true이어야 한다 (GDPR Article 17)"
        );

        // 소거 완료 후 플래그 초기화
        manager.clear_pending_deletion();
        assert!(
            !manager.has_pending_deletion(),
            "clear_pending_deletion() 이후 has_pending_deletion()은 false이어야 한다"
        );
    }

    // ── #5156: per-erasure id (restart-stable, per-erasure-distinct) ──────────

    #[test]
    fn pending_erasure_id_none_until_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        assert_eq!(
            manager.pending_erasure_id(),
            None,
            "no erasure before anything"
        );
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert_eq!(
            manager.pending_erasure_id(),
            None,
            "a grant is not an erasure"
        );
    }

    #[test]
    fn pending_erasure_id_is_the_persisted_collision_proof_nonce() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path.clone());
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();

        let id = manager
            .pending_erasure_id()
            .expect("revoke sets a pending erasure id");
        // #5165: the id is the collision-proof per-revoke nonce (NOT wall-clock), and
        // it must equal the persisted record's `erasure_nonce` so a restart recovers an
        // identical id (the fire-once gate depends on that).
        let persisted: ConsentRecord =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(id, persisted.erasure_nonce.unwrap());
        assert!(
            id.starts_with("erasure_"),
            "the id is a ULID nonce, not a timestamp: {id}"
        );
    }

    #[test]
    fn pre_upgrade_erasure_without_nonce_falls_back_to_timestamp() {
        // Migration safety (#5165): a record revoked BEFORE the `erasure_nonce` field
        // existed (no nonce in consent.json) must keep its timestamp-based id on load,
        // so its in-flight fire-once-gate identity is unchanged across the upgrade (no
        // spurious re-announce → no unbounded peer delete of post-re-grant data).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let revoked_at = Utc::now();
        let record = ConsentRecord {
            consent_id: "consent_legacy".to_string(),
            version: "1".to_string(),
            granted_at: revoked_at,
            expires_at: None,
            revoked_at: Some(revoked_at),
            data_deletion_requested: true,
            erasure_nonce: None, // pre-upgrade record: no nonce
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();

        let manager = ConsentManager::new(path);
        assert_eq!(
            manager.pending_erasure_id(),
            Some(revoked_at.to_rfc3339()),
            "a pre-upgrade erasure (no nonce) keeps its timestamp id across the upgrade"
        );
    }

    #[test]
    fn pending_erasure_id_recovered_on_restart_but_durability_deferred() {
        // The per-erasure id MUST survive restart (the sync engine's stage-2
        // fire-once gate keys on it). But durability is DEFERRED (#5156 stage-1
        // guard): `has_pending_deletion()` stays false on restart so the current
        // sync gate does NOT re-announce a DeletionEvent (which the unbounded peer
        // delete would turn into post-re-grant data loss). Stage 2 re-enables
        // durability safely via a persisted, fire-once, sender-side gate.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let before = ConsentManager::new(path.clone());
        before
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        before.revoke_consent().unwrap();
        let id_before = before.pending_erasure_id();
        assert!(id_before.is_some());

        // Simulate restart: a fresh manager loads the persisted revoked file.
        let after = ConsentManager::new(path);
        assert_eq!(
            after.pending_erasure_id(),
            id_before,
            "the per-erasure id must be recovered identically on restart (for stage 2)"
        );
        assert!(
            !after.has_pending_deletion(),
            "durability deferred: the old gate must NOT re-arm on restart (no re-announce \
             → no unbounded peer delete of post-re-grant data)"
        );
        // The LOCAL write gate must NOT be re-engaged on restart either.
        assert!(
            !after.deletion_flag().load(Ordering::Acquire),
            "restart must not re-block local writes"
        );
    }

    #[test]
    fn pending_erasure_id_changes_for_a_distinct_second_erasure() {
        // revoke → re-grant (new collection window + a disk write, so time advances)
        // → revoke again. The second erasure is genuinely distinct and must get a
        // DIFFERENT id so the sync engine re-propagates it and the ledger records it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();
        let id1 = manager.pending_erasure_id().unwrap();

        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();
        let id2 = manager.pending_erasure_id().unwrap();

        assert_ne!(id1, id2, "a distinct second erasure must get a distinct id");
    }

    #[test]
    fn regrant_keeps_the_pending_erasure_id() {
        // #4630-b: re-grant does not retract the pending erasure → the id is kept
        // (no second revoke yet, so it must NOT change).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();
        let id1 = manager.pending_erasure_id();

        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert_eq!(
            manager.pending_erasure_id(),
            id1,
            "re-grant must keep the pending erasure id (#4630-b)"
        );
    }

    #[test]
    fn revoke_without_active_consent_has_no_erasure_id() {
        // Degenerate path (no production caller — the UI requires an active consent
        // before revoke): revoking with no active consent sets the pending_deletion
        // signal but NO per-erasure id (nothing was collected → no distinct
        // erasure). Locks the documented asymmetry so a refactor can't change it
        // silently.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager.revoke_consent().unwrap();
        assert!(
            manager.has_pending_deletion(),
            "revoke still arms the pending-deletion signal"
        );
        assert_eq!(
            manager.pending_erasure_id(),
            None,
            "no active consent revoked → no per-erasure id"
        );
    }

    #[test]
    fn clear_pending_deletion_resets_the_erasure_id() {
        // Post-erasure-completion: clear must drop the id so a completed erasure is
        // not re-propagated/re-audited as if still pending.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();
        assert!(manager.pending_erasure_id().is_some());

        manager.clear_pending_deletion();
        assert_eq!(
            manager.pending_erasure_id(),
            None,
            "clear_pending_deletion must drop the per-erasure id"
        );
    }

    /// #4630 (decision b — erasure is irrevocable): re-granting consent after a revoke
    /// does NOT retract a not-yet-propagated Art. 17 erasure. `grant_consent` leaves
    /// `pending_deletion` set, so revoke→re-grant still reports pending — the prior
    /// erasure stands (and still fires the cross-device DeletionEvent on next sync)
    /// while a fresh local collection window begins. Locks the rejection of option (a).
    #[test]
    fn pending_deletion_survives_regrant_erasure_is_irrevocable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();
        assert!(
            manager.has_pending_deletion(),
            "revoke 직후 소거 대기여야 한다 (GDPR Article 17)"
        );

        // Re-grant BEFORE the erasure has propagated (e.g. no LAN peer online / sync off).
        manager
            .grant_consent(
                ConsentPermissions {
                    screen_capture: true,
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        // The fresh grant is active — a new collection window opens…
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);
        assert!(manager.effective_permissions().screen_capture);
        // …but the prior erasure is irrevocable: still pending (decision b, not a).
        assert!(
            manager.has_pending_deletion(),
            "#4630(b): 재동의는 미전파 erasure를 철회하지 않는다 — has_pending_deletion()은 true 유지"
        );
    }

    #[test]
    fn consent_permissions_cross_device_sync_default_false() {
        let perms = ConsentPermissions::default();
        assert!(
            !perms.cross_device_sync,
            "cross_device_sync must default to false (GDPR Article 6)"
        );
    }

    #[test]
    fn consent_cross_device_sync_permission_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        // Without cross_device_sync
        let perms = ConsentPermissions::default();
        manager.grant_consent(perms, 30).unwrap();
        assert!(!manager.is_permitted(|p| p.cross_device_sync));

        // With cross_device_sync
        let perms_with_sync = ConsentPermissions {
            cross_device_sync: true,
            ..Default::default()
        };
        manager.grant_consent(perms_with_sync, 30).unwrap();
        assert!(manager.is_permitted(|p| p.cross_device_sync));
    }

    #[test]
    fn consent_without_full_text_extraction_deserializes() {
        let json = r#"{"screen_capture":true,"activity_pattern_learning":true}"#;
        let perms: ConsentPermissions = serde_json::from_str(json).unwrap();
        assert!(!perms.full_text_extraction);
        assert!(perms.activity_pattern_learning);
    }

    #[test]
    fn revoke_persists_file_and_new_manager_sees_not_granted() {
        // After revoke_consent(), the consent file must still exist on disk (no
        // TOCTOU delete), but a freshly constructed ConsentManager pointing at
        // the same path must report NotGranted because data_deletion_requested
        // is set in the persisted record.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path.clone());
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);
        assert!(path.exists(), "consent file must exist after grant");

        manager.revoke_consent().unwrap();
        assert!(
            path.exists(),
            "revoke must keep the file (atomic flag, not delete)"
        );

        // Verify the on-disk record has data_deletion_requested = true.
        let raw = std::fs::read_to_string(&path).unwrap();
        let record: ConsentRecord = serde_json::from_str(&raw).unwrap();
        assert!(
            record.data_deletion_requested,
            "file must have deletion flag set"
        );

        // A new manager reading the same file must treat it as revoked.
        let new_manager = ConsentManager::new(path.clone());
        assert_eq!(
            new_manager.check_consent(),
            ConsentStatus::NotGranted,
            "new manager must see NotGranted for a revoked record"
        );
    }

    #[test]
    fn consent_permissions_legacy_json_without_cross_device_sync() {
        // Records written before cross_device_sync was added must deserialize.
        let legacy_json = r#"{
            "screen_capture": true,
            "ocr_processing": false,
            "telemetry": true,
            "process_monitoring": true,
            "input_activity": false,
            "window_title_collection": false,
            "app_usage_analytics": false,
            "clipboard_monitoring": false,
            "file_access_monitoring": false,
            "activity_pattern_learning": false
        }"#;
        let perms: ConsentPermissions = serde_json::from_str(legacy_json).unwrap();
        assert!(perms.screen_capture);
        assert!(
            !perms.cross_device_sync,
            "missing field must default to false"
        );
    }

    #[test]
    fn consent_permissions_legacy_json_without_microphone() {
        // Records written before the microphone tier (Tier 8) was added must
        // deserialize, defaulting microphone to false (fail-closed): a pre-Tier-8
        // user who had screen consent must NOT have the mic silently authorized.
        let legacy_json = r#"{
            "screen_capture": true,
            "ocr_processing": false,
            "telemetry": true,
            "process_monitoring": true,
            "input_activity": false,
            "window_title_collection": false,
            "app_usage_analytics": false,
            "clipboard_monitoring": false,
            "file_access_monitoring": false,
            "activity_pattern_learning": false,
            "cross_device_sync": true,
            "full_text_extraction": false,
            "memory_graph_enrichment": false
        }"#;
        let perms: ConsentPermissions = serde_json::from_str(legacy_json).unwrap();
        assert!(perms.screen_capture);
        assert!(
            !perms.microphone,
            "missing microphone field must default to false (fail-closed)"
        );
    }

    #[test]
    fn shared_arc_observes_writes_through_other_clone() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = Arc::new(ConsentManager::new(path));
        let clone_a = Arc::clone(&mgr);
        let clone_b = Arc::clone(&mgr);
        // Write through clone_a (now &self), observe through clone_b — proves the
        // shared immutable Arc is interior-mutable (the whole point).
        clone_a
            .grant_consent(
                ConsentPermissions {
                    screen_capture: true,
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        assert!(clone_b.is_permitted(|p| p.screen_capture));
        assert_eq!(clone_b.check_consent(), ConsentStatus::Valid);
        clone_a.revoke_consent().unwrap();
        assert_eq!(clone_b.check_consent(), ConsentStatus::NotGranted);
        assert!(clone_b.has_pending_deletion());
    }

    #[test]
    fn effective_permissions_all_false_unless_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path.clone());
        // Absent → all false.
        assert!(!mgr.effective_permissions().screen_capture);
        // Granted (Valid) → live.
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        assert!(mgr.effective_permissions().screen_capture);

        // Expired → all false even though the boolean is true. Craft a record on disk
        // (grant_consent can't synthesize Expired: it hardcodes expires_at:None).
        let expired = ConsentRecord {
            consent_id: "exp-1".into(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(2),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(&path, serde_json::to_string(&expired).unwrap()).unwrap();
        let mgr2 = ConsentManager::new(path.clone());
        assert_eq!(mgr2.check_consent(), ConsentStatus::Expired);
        assert!(
            !mgr2.effective_permissions().screen_capture,
            "Expired → effective all-false"
        );

        // UpdateRequired (version mismatch) → all false. Clear expires_at so the
        // version-mismatch branch is reached: check_consent evaluates expiry BEFORE
        // version, and `expired` above is already past its expiry (would short-
        // circuit to Expired and never test the UpdateRequired path).
        let stale = ConsentRecord {
            version: "0.0.1".into(),
            expires_at: None,
            ..expired
        };
        std::fs::write(&path, serde_json::to_string(&stale).unwrap()).unwrap();
        let mgr3 = ConsentManager::new(path);
        assert_eq!(mgr3.check_consent(), ConsentStatus::UpdateRequired);
        assert!(
            !mgr3.effective_permissions().screen_capture,
            "UpdateRequired → effective all-false"
        );
    }

    /// #4928: deletion_flag 가 revoke→true / grant→false / clear→false 로
    /// pending_deletion 라이프사이클을 미러링하는지 검증한다.
    #[test]
    fn deletion_flag_mirrors_revoke_grant_clear_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path);
        let flag = mgr.deletion_flag();

        // 초기: false (쓰기 허용).
        assert!(
            !flag.load(Ordering::Acquire),
            "신규 ConsentManager 의 deletion_flag 는 false 여야 한다"
        );

        // grant: false 유지.
        mgr.grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert!(
            !flag.load(Ordering::Acquire),
            "grant 후 deletion_flag 는 false 여야 한다"
        );

        // revoke: true (erase 직전 set).
        mgr.revoke_consent().unwrap();
        assert!(
            flag.load(Ordering::Acquire),
            "revoke 후 deletion_flag 는 true 여야 한다 (#4928 erasure backstop)"
        );

        // clear_pending_deletion: false (소거 완료 후 쓰기 재개).
        mgr.clear_pending_deletion();
        assert!(
            !flag.load(Ordering::Acquire),
            "clear_pending_deletion 후 deletion_flag 는 false 여야 한다"
        );
    }

    /// #4928: revoke 후 재동의(grant)는 LOCAL deletion_flag 를 clear 해 쓰기를
    /// 재개하지만, REMOTE 전파 신호 `pending_deletion` 은 #4630(b) 에 따라 true 를
    /// 유지한다 — 두 신호가 별개임을 단언한다.
    #[test]
    fn deletion_flag_clears_on_regrant_but_pending_deletion_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path);
        let flag = mgr.deletion_flag();

        mgr.grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        mgr.revoke_consent().unwrap();
        assert!(flag.load(Ordering::Acquire));
        assert!(mgr.has_pending_deletion());

        // 재동의: 로컬 쓰기 게이트는 열리고…
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        assert!(
            !flag.load(Ordering::Acquire),
            "재동의 후 deletion_flag 는 false — 로컬 수집 윈도우 재개"
        );
        // …미전파 erasure 는 여전히 살아있다 (#4630 b).
        assert!(
            mgr.has_pending_deletion(),
            "재동의는 미전파 erasure(pending_deletion)를 철회하지 않는다"
        );
    }

    /// #4928 round-3 (FIX B): `grant_consent`/`clear_pending_deletion`/`revoke_consent`
    /// 는 모두 `erasing` 신호를 건드리지 못한다 — `erasing` 은 erase 만 set/clear 한다.
    /// 따라서 erase 윈도우(`erasing=true`) 안에 재동의가 끼어들어도 `erasing` 은 true 를
    /// 유지하고, write funnel 의 `deletion_flag || erasing` 술어가 쓰기를 계속 스킵한다.
    #[test]
    fn erasing_is_not_cleared_by_grant_or_clear_or_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path);
        let erasing = mgr.erasing();
        let deletion = mgr.deletion_flag();

        // erase 가 시작된 것을 시뮬레이션: erasing=true (RAII 가드가 set 하는 신호).
        erasing.store(true, Ordering::Release);

        // 재동의: deletion_flag 는 clear 되지만 erasing 은 그대로여야 한다.
        mgr.grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert!(
            !deletion.load(Ordering::Acquire),
            "grant 후 deletion_flag 는 clear 된다"
        );
        assert!(
            erasing.load(Ordering::Acquire),
            "grant_consent 는 erasing 을 clear 하지 못한다 (TOCTOU 차단)"
        );

        // clear_pending_deletion 도 erasing 을 건드리지 않는다.
        mgr.clear_pending_deletion();
        assert!(
            erasing.load(Ordering::Acquire),
            "clear_pending_deletion 는 erasing 을 clear 하지 못한다"
        );

        // revoke 도 erasing 을 set 하지 않는다(erase 만 set). 이미 true 이므로 그대로.
        mgr.revoke_consent().unwrap();
        assert!(
            erasing.load(Ordering::Acquire),
            "revoke 는 erasing 을 건드리지 않는다 (값 유지)"
        );
    }

    /// #4928 round-3: 신규 ConsentManager 의 `erasing` 은 false 이며, `erasing()` 은
    /// 동일 Arc 를 반환한다(composition root 가 SQLite/frames 에 install 할 공유 셀).
    #[test]
    fn erasing_default_false_and_shared_ptr_eq() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ConsentManager::new(dir.path().join("consent.json"));
        let a = mgr.erasing();
        let b = mgr.erasing();
        assert!(
            !a.load(Ordering::Acquire),
            "신규 erasing 은 false 여야 한다"
        );
        assert!(
            Arc::ptr_eq(&a, &b),
            "erasing() 는 동일 Arc 를 반환해야 한다 (ptr-eq)"
        );
    }

    /// #4928: 공유 Arc 를 통해 한 clone 이 set 한 flag 를 다른 clone 이 관측한다
    /// (deletion_flag() 가 동일 셀을 가리키는지 — ptr-eq).
    #[test]
    fn deletion_flag_shared_through_arc_clones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = Arc::new(ConsentManager::new(path));
        let flag_a = mgr.deletion_flag();
        let flag_b = Arc::clone(&mgr).deletion_flag();
        assert!(
            Arc::ptr_eq(&flag_a, &flag_b),
            "deletion_flag() 는 동일 Arc 를 반환해야 한다 (ptr-eq)"
        );
        mgr.grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        mgr.revoke_consent().unwrap();
        assert!(
            flag_b.load(Ordering::Acquire),
            "한 핸들의 revoke 가 다른 핸들이 보유한 동일 flag 에 반영되어야 한다"
        );
    }

    #[test]
    fn status_and_permissions_atomic_returns_raw_not_effective() {
        // status_and_permissions()는 한 가드 안에서 상태 + 원본 권한을 반환한다.
        // 핵심 단언: 비-Valid(Expired) 상태에서도 권한을 0으로 만들지 않고
        // 원본 부여 권한을 그대로 돌려줘야 한다 (effective_permissions와 대비).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");

        // (1) no consent → (NotGranted, default-all-false).
        let mgr = ConsentManager::new(path.clone());
        let (status, perms) = mgr.status_and_permissions();
        assert_eq!(status, ConsentStatus::NotGranted);
        assert!(
            !perms.screen_capture,
            "no consent → permissions default to all-false"
        );

        // (2) Valid + granted → (Valid, granted perms).
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        let (status, perms) = mgr.status_and_permissions();
        assert_eq!(status, ConsentStatus::Valid);
        assert!(perms.screen_capture, "Valid → live granted permissions");

        // (3) Expired record → (Expired, RAW granted perms — NOT zeroed).
        // grant_consent can't synthesize Expired (hardcodes expires_at: None), so
        // craft the record onto disk and reconstruct the manager from it.
        let expired = ConsentRecord {
            consent_id: "exp-sp-1".into(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(2),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(&path, serde_json::to_string(&expired).unwrap()).unwrap();
        let mgr_expired = ConsentManager::new(path);
        let (status, perms) = mgr_expired.status_and_permissions();
        assert_eq!(status, ConsentStatus::Expired);
        assert!(
            perms.screen_capture,
            "Expired → status_and_permissions returns RAW granted perms (NOT zeroed like effective_permissions)"
        );
        // Contrast: effective_permissions zeroes on non-Valid.
        assert!(
            !mgr_expired.effective_permissions().screen_capture,
            "sanity: effective_permissions DOES zero on Expired (the distinction we preserve)"
        );
    }
}
