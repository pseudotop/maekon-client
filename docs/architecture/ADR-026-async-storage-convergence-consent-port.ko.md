[English](./ADR-026-async-storage-convergence-consent-port.md) | [한국어](./ADR-026-async-storage-convergence-consent-port.ko.md)

# ADR-026: 비동기 스토리지 수렴 + 객체 안전(object-safe) ConsentManagerPort

**Status**: Accepted
**Date**: 2026-06-04
**Scope**: `crates/maekon-core/src/ports/focus_storage.rs`, `crates/maekon-core/src/ports/web_storage.rs`, `crates/maekon-core/src/ports/annotation_storage.rs`, `crates/maekon-core/src/consent.rs`, `crates/maekon-storage/src/sqlite/`, `crates/maekon-web/`, `src-tauri/src/focus_analyzer/`
**Related**: ADR-001 (§2 async-trait, §6 의존성 방향, §7 포트 배치), ADR-007 (async 런타임 안전성), ADR-021 (config/consent core 배치), ADR-024 (대화 콘텐츠 가드 포트)
**Implementation**: 완료 — 아래 "구현 완료" 노트 참조.

> **Issue**: E20-19 / #4811 (XL). #4928(consent-erasure drain barrier)에 의존해 보류되었으나, 해당 이슈가 **머지·CLOSE** 되어 의존성이 해소됨 — 따라서 본 ADR 작성이 가능해졌다.

---

## 구현 완료

§Decision 2 의 슬라이스 마이그레이션이 **PR-1..PR-9** 로 출하되어, 동기 스토리지 표면 전체(FocusStorage + `WebStorage` 14개 하위 트레잇 + AnnotationStorage)를 `#[async_trait]` 로 전환하고 모든 메서드를 `with_conn`/`with_conn_mut`/`with_conn_read` 펀넬(`spawn_blocking`)로 라우팅했다. 따라서 단일-커넥션 `parking_lot` 가드는 `.await` 를 가로질러 보유되지 않는다(#4928 erase barrier 보존):

- **PR-1** — 객체 안전 `ConsentManagerPort`(§1) + `impl for ConsentManager`(가산, 호출처 변경 없음).
- **PR-2** — `FocusStorage` → async; `focus_analyzer` 20 호출처에 `.await`.
- **PR-3** — `AnnotationStorage` → async(웹 최소 하위 트레잇; per-sub-trait 레시피 증명).
- **PR-4/5/6** — `FrameQueryStorage`/`EventQueryStorage`/`StorageMaintenanceStorage` + `TagStorage`/`ActivityStatsStorage`/`FocusQueryStorage` + `SuggestionQueryStorage`/`DigestStorage` → async; 단일 `suggestion_digest_storage.rs` `block_in_place` 제거.
- **PR-7** — `BackupStorage`/`SegmentQueryStorage`/`GuiInteractionStorage` → async.
- **PR-8** — `CoachingQueryStorage`/`HabitStorage` → async.
- **PR-9** — `DashboardStreamingStorage` → async; `WebStorage` 슈퍼트레잇 + blanket impl 이 이제 **균일 async**(스토리지 전체 표면에 ADR-001 §2 충족).
- **PR-final** — 본 문서 정리 + `DigestStorage` async 본문의 미래 `block_in_place` 재도입을 막는 `maekon-storage` current-thread `#[tokio::test]` 가드(이 본문은 `maekon-web` 엔트리포인트가 없어 핸들러 테스트가 커버하지 못함).

**Follow-up #1 (미해소)**: 구체 `Arc<ConsentManager>` 41개 소비자를 `Arc<dyn ConsentManagerPort>` 로 옮기는 **41-소비자 마이그레이션**은 본 ADR 범위 밖이다 — PR-1 은 포트만 도입했다. 아래 Known Follow-up #1 로 추적한다.

---

## Context

스토리지 레이어가 ADR-001 §2("모든 포트 트레잇은 async")와 §7("크로스 크레이트 계약은 구체 타입이 아닌 포트")를 충족하지 못하게 막는 두 가지 얽힌 설계 문제가 있다. 두 문제가 얽힌 이유: consent 권위체(`ConsentManager`)는 스토리지 레이어의 GDPR-critical 형제이며, 둘 다 #4928 erasure barrier 를 공유한다. 따라서 consent 를 함께 고려하지 않고 스토리지만 async 로 바꾸면 그 barrier 를 회귀시킬 위험이 있다.

### 문제 1 — 동기 스토리지 포트

`StorageService` 와 `MetricsStorage`(`crates/maekon-core/src/ports/storage.rs`)는 **이미 `#[async_trait]`** 이다. 그러나 세 포트 패밀리는 여전히 **동기**(plain `fn`, `#[async_trait]` 없음)다:

| 포트 패밀리 | 파일 | 메서드 | async? |
|-------------|------|-------:|--------|
| `FocusStorage` | `crates/maekon-core/src/ports/focus_storage.rs` | 12 | sync |
| `WebStorage` 하위 트레잇 (14개) | `crates/maekon-core/src/ports/web_storage.rs` | 65 | sync |
| `AnnotationStorage` | `crates/maekon-core/src/ports/annotation_storage.rs` | 3 | sync |
| **동기 표면 합계** | | **80** | |

`WebStorage` 는 **합성 슈퍼트레잇**이다: 이미-async 인 `StorageService` + `MetricsStorage` **와** 14개 동기 하위 트레잇(`TagStorage`, `FrameQueryStorage`, `EventQueryStorage`, `StorageMaintenanceStorage`, `ActivityStatsStorage`, `FocusQueryStorage`, `SuggestionQueryStorage`, `DigestStorage`, `BackupStorage`, `GuiInteractionStorage`, `SegmentQueryStorage`, `CoachingQueryStorage`, `HabitStorage`, `DashboardStreamingStorage`) 및 `AnnotationStorage` 를 상속한다. 즉 `WebStorage` 는 오늘 **sync/async 혼합 합성**이며, 이는 ADR-001 §2 가 금지하는 자기-비일관 표면이다.

**왜 단순 미관 문제가 아니라 런타임 결함인가.** 단일 SQLite `Connection` 은 `GuardedConnection`(`crates/maekon-storage/src/sqlite/guarded_connection.rs`) 안의 `parking_lot::Mutex` 로 보호된다. async impl(`StorageService`/`MetricsStorage`)은 `SqliteStorage::with_conn` / `with_conn_read` 를 경유하며, 이들은 `tokio::task::spawn_blocking` 으로 오프로드하여 parking_lot 가드가 **블로킹 풀 스레드**에서 획득되고 `.await` 를 가로질러 보유되지 않게 한다(#4928 설계). 그러나 동기 impl 은 그렇지 않다: 예) `SqliteStorage::increment_focus_metrics`(`crates/maekon-storage/src/sqlite/edge_intelligence/focus_metrics.rs:146`)는 `self.conn.write_lock().run(...)` 를 직접 호출하여 **호출한 스레드** 위에서 락을 보유한다. 그 호출자가 async 컨텍스트면 SQLite I/O 가 tokio 워커 스레드를 블로킹한다.

두 호출 형태가 이에 걸린다:

1. **`focus_analyzer`(src-tauri)** — `FocusAnalyzer` 는 `Arc<dyn FocusStorage>` 를 보유하고 `async fn`(`on_app_switch`, `analyze_periodic`, `on_idle_resume`)에서 호출한다. **20개 호출처**(`src-tauri/src/focus_analyzer/mod.rs` 11개, `src-tauri/src/focus_analyzer/suggestions.rs` 9개)가 동기 `FocusStorage` 메서드를 async fn 내부에서 직접 호출하며, 각 호출이 SQLite 쓰기 동안 런타임 스레드를 블로킹한다.
2. **`maekon-web` 핸들러** — Axum 핸들러가 `Arc<dyn WebStorage>`(`crates/maekon-web/src/app_state.rs:47`, `web_contexts`·`grpc/*`·`services/*` 로 전파)를 소비한다. 핸들러 표면에는 `crates/maekon-web/src` 전반에 **~56개 `storage.<method>(...)` 호출 표현식**이 있다. 웹 크레이트는 **마이그레이션 진행 중**이다: 최신 핸들러는 이미 동기 호출을 `tokio::task::spawn_blocking` 으로 감싼다(예: `handlers/annotations.rs`, `handlers/coaching.rs`, `services/data_web_service.rs`, `services/search_service.rs`). 반면 잔존 소수는 여전히 동기 메서드를 직접 또는 레거시 `block_in_place` 브리지로 호출한다. 스토리지 크레이트 자체에는 스토리지 관련 `block_in_place` 가 정확히 **1개**(`crates/maekon-storage/src/sqlite/web_storage_impl/suggestion_digest_storage.rs`) 있고, 별개의 **무관한** `block_in_place` 패밀리가 `CoachingPort`(`crates/maekon-core/src/ports/coaching.rs`, `crates/maekon-analysis/src/coaching_engine/port_impl.rs`)에 있는데 이는 SQLite 가 아니라 `tokio::sync::RwLock` 을 브리지하며 이미 비블로킹 async 변형(F-RR-C37-01)을 갖고 있어 **본 ADR 범위 밖**이다.

> **정직한 blast-radius 재측정.** 이슈/이전 스코프는 39–64 사이트로 추정했다. 정정된 수치: async 를 받을 **동기 트레잇 메서드 80개**, **`FocusStorage` 호출처 20개**(전부 `focus_analyzer`), `maekon-web/src` 의 **`WebStorage` 호출 표현식 ~56개**, `maekon-storage` 의 **동기 impl 메서드 80개**(`focus_storage_impl.rs` + `web_storage_impl/*` + `annotation_storage_impl.rs`), 범위 내 스토리지 `block_in_place` **1개**. "39–64" 범위는 웹 하위 트레잇 메서드 총합을 과소(80 > 64)·`block_in_place` 를 과대평가했다(웹 호출자 대부분 이미 `spawn_blocking` 전환). 지배적 비용은 호출처 `.await` 삽입이 아니라 **메서드 시그니처 변경**(80 시그니처 × N impl)이다.

### 문제 2 — `ConsentManager` 가 포트가 아닌 구체 타입

`ConsentManager`(`crates/maekon-core/src/consent.rs`, ~1183줄)는 `src-tauri` 및 어댑터 크레이트 전반의 **41개** `Arc<ConsentManager>` / `&ConsentManager` 사이트(scheduler 루프, sync engine, vision privacy gateway, provider 가드, web)에서 **구체 타입**으로 소비된다. ADR-001 §7 은 크로스 크레이트 계약을 포트 뒤로 두길 원한다. ADR-021 은 의도적으로 `ConsentManager` 를 **`maekon-core` 에 유지**한다(경계 예외: 인프라가 아닌 로컬 제품-정책 상태). 따라서 본 ADR 은 그것을 **이동하지 않고**, ADR-001 §7 의 "1개 초과 크레이트가 소비 ⇒ core" 규칙을 충족하며 core 안에 포트를 *나란히* 추가한다.

**블로커.** 이전 스펙은 `ConsentManagerPort` 를 제안했으나 핵심 메서드가:

```rust
fn is_permitted(&self, check: impl Fn(&ConsentPermissions) -> bool) -> bool;
```

이는 **객체 안전하지 않다**(Rust 2024: "not dyn compatible"): 메서드의 제네릭 타입 파라미터(`impl Fn`)는 그 메서드가 vtable 에 들어갈 수 없음을 뜻하므로 `Arc<dyn ConsentManagerPort>` 가 컴파일되지 않는다. 본 ADR 을 위해 실증 확인 — 후보 트레잇에 그 정확한 시그니처를 추가하면:

```
error[E0038]: the trait `ConsentManagerPort` is not dyn compatible
note: for a trait to be dyn compatible it needs to allow building a vtable
      ...method `is_permitted` has generic type parameters
```

`Arc<dyn ConsentManagerPort>` 없이는 포트가 DI 패턴(ADR-001 §3)에 무용하다.

**결정적 측정.** `is_permitted` 는 **production-public 이지만 테스트에서만 호출**된다: 6개 호출처 전부 `consent.rs` 의 `#[cfg(test)] mod tests` 블록(414줄 이후)에 있다. 프로덕션 consent 게이팅은 이미 `effective_permissions()`(fail-closed 유효성 검증)와 `status_and_permissions()` 를 경유하며 `is_permitted` 는 절대 쓰지 않는다. 그러므로 `is_permitted` 는 포트에 있을 필요가 전혀 없다.

### 왜 지금인가

#4928(consent-erasure drain barrier + `GuardedConnection` chokepoint + `deletion_flag`/`erasing` 신호)이 **머지·CLOSE** 되었다. 이로써 원래의 보류 사유가 사라졌다: erasure barrier 가 이제 안정적인 단일 chokepoint(`SqliteStorage::with_conn`/`write_lock`, `ConsentManager::deletion_flag()`/`erasing()`)이므로, in-flight barrier 와 경쟁하는 대신 known-good barrier *위에서* async 전환을 설계할 수 있다.

## Decision

### 1. `maekon-core` 의 객체 안전 `ConsentManagerPort`

`crates/maekon-core/src/ports/consent_manager.rs` 에 **객체 안전**(`dyn`-호환) 포트를 도입한다. (ADR-021 에 따라 core 에 유지되는) `ConsentManager` 가 이를 구현한다. 트레잇은 **동기**다 — `ConsentManager` 는 순수 인메모리 `parking_lot::RwLock` 상태 + 로컬 JSON 파일 I/O(오늘 어디에도 `.await` 없음)이며, ADR-021 은 그것이 async 외부 부수효과를 갖는 것을 금지한다. ADR-001 §2 의 `#[async_trait]` 규칙은 I/O-bound 포트를 겨냥하며, async 표면이 없는 consent **정책** 권위체는 (기존 동기 정책 접근자들과 일관되게) 올바르게 동기 포트다.

```rust
/// 객체 안전(dyn-호환) consent 권위체 포트.
/// `ConsentManager`(ADR-021 에 따라 maekon-core 에 유지)가 구현한다.
pub trait ConsentManagerPort: Send + Sync {
    fn check_consent(&self) -> ConsentStatus;
    fn current_consent(&self) -> Option<ConsentRecord>;
    /// Fail-closed: consent 가 현재 Valid 일 때만 권한을 반환하고, 그 외에는
    /// `ConsentPermissions::default()`(전부 false)를 반환. 제거된 제네릭
    /// `is_permitted` 대신 사용하는 정규 게이팅 접근자.
    fn effective_permissions(&self) -> ConsentPermissions;
    /// UI 용 원자적 (status, raw-permissions) 스냅샷(fail-closed 게이팅 아님).
    fn status_and_permissions(&self) -> (ConsentStatus, ConsentPermissions);
    fn grant_consent(
        &self,
        permissions: ConsentPermissions,
        data_retention_days: u32,
    ) -> Result<(), CoreError>;
    fn revoke_consent(&self) -> Result<(), CoreError>;
    fn has_pending_deletion(&self) -> bool;
    fn clear_pending_deletion(&self);
    /// #4928 erasure-barrier 신호(스토리지 어댑터에 install 되는 공유 `Arc`).
    fn deletion_flag(&self) -> Arc<AtomicBool>;
    fn erasing(&self) -> Arc<AtomicBool>;

    /// 편의: 가장 흔한 `is_permitted(|p| p.<field>)` 테스트 관용구를 대체하는
    /// 비제네릭·객체안전 메서드. `effective_permissions()` 위에 default 구현되어
    /// impl 이 공짜로 얻으며, 제네릭 파라미터가 없어 vtable 에 남는다.
    fn telemetry_permitted(&self) -> bool {
        self.effective_permissions().telemetry
    }
    fn screen_capture_permitted(&self) -> bool {
        self.effective_permissions().screen_capture
    }
}
```

**`is_permitted` 수정(과 원본이 객체 안전하지 않았던 이유).** 원본 `fn is_permitted(&self, check: impl Fn(&ConsentPermissions) -> bool) -> bool` 는 제네릭 타입 파라미터(`impl Fn` 은 `<F: Fn(...)>` 로 desugar)를 갖는다. `dyn Trait` vtable 은 메서드당 *고정된* 시그니처의 항목을 하나 갖는데, 제네릭 메서드는 인스턴스화하는 클로저 타입마다 별도 vtable 항목이 필요해 불가능하므로 컴파일러가 트레잇을 dyn-비호환(`E0038`)으로 거부한다. **수정: `is_permitted` 를 트레잇에서 완전히 제거한다.** 그것은 production-public 이지만 테스트 전용이므로:

- `ConsentManager::is_permitted` 를 구체 타입의 **인헌트(inherent) 메서드**로 유지한다(변경 없음 — 6개 in-crate 테스트는 `dyn` 이 아니라 `ConsentManager` 에 직접 계속 호출). 프로덕션 코드도 포트 소비자도 이를 필요로 한 적이 없다.
- "권한 X 가 현재 부여됐는가"를 알고 싶은 미래 호출자는 **객체 안전한** `effective_permissions()` 를 호출해 필드를 읽거나, 비제네릭 `*_permitted()` default 헬퍼를 쓴다. 호출자는 `ConsentPermissions` 스냅샷(`Clone` 임)을 검사하며, 이는 단일 predicate 클로저보다 엄격히 더 유연하고 완전히 `dyn`-호환이다.

**검증.** 이 정확한 표면의 scratch 트레잇 파일을 `crates/maekon-core/src/ports/mod.rs` 에 배선하고 `ConsentManager` 에 구현한 뒤 `cargo check -p maekon-core` 로 (a) 트레잇 + `Arc<ConsentManager> -> Arc<dyn ConsentManagerPort>` 강제 변환 컴파일, (b) 제네릭 `is_permitted` 재추가 시 `E0038 ... is not dyn compatible` 발생을 확인했다. 이후 scratch 파일을 제거하여 본 ADR 커밋은 **문서 전용**이다.

**근거**: 객체 안전성은 `Arc<dyn _>` DI(ADR-001 §3)에 필수다. `ConsentPermissions` 스냅샷 접근자는 최소·비제네릭·미래 대응형 형태이며, 클로저 형태는 인헌트 메서드가 보존하는 약간 더 간결한 테스트 관용구 외에 프로덕션 이점이 없었다.

### 2. 비동기 스토리지 수렴 — 슬라이스 마이그레이션

80개 동기 메서드를 `#[async_trait]` `async fn` 으로 전환하고 블로킹 호출 형태를 제거하되, **독립 출하 가능한 슬라이스**로 나눈다. 각 슬라이스는 단일 `cargo check -p <crate>` 를 green 으로 유지하고 독립 리뷰 가능하다. 순서는 **leaf 소비자를 공유 슈퍼트레잇보다 먼저** 전환하고, 합성 슈퍼트레잇 `WebStorage` 를 마지막에 두어 모든 하위 트레잇이 async 가 된 뒤에만 blanket impl 이 한 번 뒤집히게 함으로써 lock-ordering 위험을 최소화한다.

| PR | 범위 | 헤드리스 검증? | 비고 |
|----|------|----------------|------|
| **PR-1** | `ConsentManagerPort`(§1) + `impl for ConsentManager` 추가. 순수 가산 — 호출처 변경 **0**(기존 41 구체 소비자 무수정). | ✅ `cargo check -p maekon-core` + `Arc<dyn ConsentManagerPort>` 강제변환 검증 신규 유닛 테스트 | 객체 안전 포트를 먼저 출하; 문제 2 를 스토리지 변경에서 분리. |
| **PR-2** | `FocusStorage`(12 메서드) → `#[async_trait]`. `focus_storage_impl.rs` 를 async `with_conn`/`with_conn_read` 위임 `async fn` 으로. `focus_analyzer` 의 **20** 호출처(이미 `async fn`)에 `.await` 추가. | ✅ `cargo check -p maekon-storage -p maekon-app` + 기존 `focus_analyzer` `#[tokio::test]` | 자기완결: `FocusStorage` 의 유일 소비자는 `focus_analyzer`. 제거할 `block_in_place` 없음(직접 동기 호출). |
| **PR-3** | `AnnotationStorage`(3 메서드) → async. `annotation_storage_impl.rs` + `handlers/annotations.rs`(이미 `spawn_blocking` — 직접 `.await` 로 교체). | ✅ `cargo check -p maekon-web` + annotation 핸들러 테스트 | 최소 웹 하위 트레잇; 스케일 전 per-sub-trait 레시피 증명. |
| **PR-4..N** | 남은 **`WebStorage` 하위 트레잇별 1 PR**(14 트레잇, ~65 메서드 — 예: PR-4 `TagStorage`, PR-5 `FrameQueryStorage`, … 응집적 하위 트레잇을 ~6–8 PR 로 묶어 각 PR ≤ ~10 메서드). 각: 하위 트레잇을 `#[async_trait]` 로, 해당 `web_storage_impl/*` 블록을 `with_conn`/`with_conn_read` 위 `async fn` 으로 전환, 대응 핸들러 `spawn_blocking` 래퍼를 직접 `.await` 로, 단일 `suggestion_digest_storage.rs` `block_in_place` 제거. | ✅ PR 별 `cargo check -p maekon-storage -p maekon-web` + 하위 트레잇 contract test(ADR-001 §8) | blanket `impl<T> WebStorage for T` 는 슈퍼트레잇 bound 목록 불변이므로 전 과정 유효; 각 하위 트레잇 메서드 본문만 async 화. `StorageService`/`MetricsStorage` 가 *이미* async 이므로 진행 중 혼합 발생 없음. |
| **PR-final** | 문서 정리: `WebStorage` doc 을 full-async 로 표기; ADR-026 Status → Accepted; ADR-001 §2 "Scope" 노트 갱신(FocusStorage/WebStorage now async). | ✅ `cargo doc` / `cargo check` | 동작 변경 없음. |

**순서 근거(lock-ordering 안전).** 오늘 동기 impl 은 `write_lock()`/`read_lock()` 을 **호출 스레드에서 동기로** 잡는다; 전환 후에는 기존 `with_conn*` 펀넬을 통해 **`spawn_blocking` 안에서** 잡는다(동일 primitive, 다른 스레드). 모든 전환 메서드가 **단일** `GuardedConnection` parking_lot 뮤텍스(락은 정확히 1개; 두 번째 락 도입 없음)를 경유하므로 **새 lock-ordering 엣지가 없다** — 전환이 기존에 없던 데드락 사이클을 만들 수 없다. leaf 소비자 우선 전환(FocusStorage → AnnotationStorage → 기타 하위 트레잇 → WebStorage doc)은 각 PR 의 blast radius 를 1개 impl 블록 + 직접 호출자로 한정한다.

**헤드리스 vs 런타임.** 컴파일 정합성, contract test(ADR-001 §8), 기존 `#[tokio::test]` 핸들러/analyzer 스위트는 모두 **헤드리스 검증 가능**(`cargo check`/`cargo test`). **헤드리스 불가**: 전환이 실제로 tokio-워커 starvation 을 *완화*하는지 — 이는 현실적 capture+web 부하 아래 실행 중인 클라이언트에서만 관측 가능한 scheduler-contention / `spawn_blocking`-풀-saturation 속성이다. 본 ADR 은 측정된 처리량 이득을 주장하지 않으며, 정당화는 벤치마크가 아닌 정합성(ADR-001 §2 + ADR-007 의 런타임 비블로킹)이다.

## Consequences

### Positive

- `WebStorage` 가 균일 async 포트가 되어 스토리지 전체 표면에 ADR-001 §2 충족.
- `focus_analyzer` 의 런타임-스레드 블로킹 결함(async fn 내 20 동기 SQLite 호출) 제거 — SQLite 작업이 `spawn_blocking` 풀로 이동(ADR-007 정렬).
- `ConsentManagerPort` 로 41개 구체 `ConsentManager` 소비자가 점진적으로 `Arc<dyn ConsentManagerPort>` 로 이전 가능하고, 실 파일-backed 매니저 없이 어댑터 테스트에서 consent **mock** 가능.
- 마이그레이션이 ~10개 작은 PR 로 출하 가능하며 각각 green·독립 revert 가능 — big-bang 스토리지 재작성 없음.

### Negative

- 80개 메서드 시그니처 + impl 변경(`async fn` + `.await`); 다수 PR 에 분산된 대형 기계적 diff.
- `spawn_blocking` 이 스레드 hop + (saturation 시) 풀 back-pressure 추가; **헤드리스 검증 불가** — 스테이징에서 관찰 필요.
- 잔존 `is_permitted` 인헌트 메서드가 테스트 전용으로 `ConsentManager` 에 남음; 미래 독자가 의도적으로 포트에 *없음*을 이해해야 함.
- PR-1→마이그레이션 동안 consent 접근 방식 2종 공존: 구체 `Arc<ConsentManager>` 와 `Arc<dyn ConsentManagerPort>`. 의도된 가산 롤아웃이나 과도기 분기점.

### Neutral

- `ConsentManager`/`ConsentPermissions`/`ConsentRecord` 는 `maekon-core` 에 유지(ADR-021 불변). 포트는 이동이 아닌 가산.
- `CoachingPort` 의 `block_in_place` 브리지는 무수정(별개 `tokio::sync::RwLock` 사안, 이미 async 변형 존재).

### Risks & mitigations (GDPR-critical)

- **#4928 erasure-barrier 민감도.** `consent.rs` 는 GDPR-critical: `deletion_flag`/`erasing` 신호와 `GuardedConnection` write-skip predicate(`deletion_flag || erasing`)가 right-to-erasure backstop 이다. async 전환은 predicate 나 `with_conn`/`write_lock` chokepoint 를 **변경하면 안 되며** — parking_lot 가드를 *어느 스레드가* 보유하는지만 바꾼다(async impl 은 이미 `spawn_blocking`). **완화**: 각 스토리지 전환 PR 은 모든 쓰기를 `with_conn`/`with_conn_mut` 로 라우팅(호출 스레드에서 bare `write_lock` 금지)해야 하고, 기존 #4928 erase-barrier 테스트(`commands/consent.rs` erase 테스트 + ptr-eq `deletion_flag`/`erasing` install 테스트)가 green 유지되어야 함 — 이것이 회귀 게이트다.
- **Lock-ordering 감사.** 단일 `GuardedConnection` 뮤텍스 ⇒ 새 ordering 엣지 없음(§2 근거 참조). 그럼에도 각 PR 은 어떤 메서드도 parking_lot 가드를 `.await` 가로질러 보유하지 않음을 확인해야 함(불가능 — 가드가 `spawn_blocking` 클로저 안에 있음) — 이는 #4928 이 확립한 B2 불변식이다.
- **테스트 전략.** (a) 전환 하위 트레잇별 ADR-001 §8 contract test; (b) 모든 `focus_analyzer` + 웹 핸들러 `#[tokio::test]` green 유지; (c) PR-1 이 `dyn ConsentManagerPort` 강제변환 + fail-closed `effective_permissions()` 테스트 추가; (d) #4928 erase-barrier 테스트가 매 스토리지 PR 의 GDPR 회귀 게이트.
- **Rollback.** 슬라이스 단위: 단일 PR revert(각각 독립). `ConsentManagerPort`(PR-1)는 순수 가산이라 revert 가 기존 구체 소비자를 깨뜨릴 수 없음.
- **정직한 한계.** scheduler-contention / `spawn_blocking`-풀-saturation 동작은 **헤드리스 검증 불가**이며, 따라서 pre-merge 게이트가 아닌 post-merge 스테이징-관찰 항목으로 수용한다.

## Alternatives Considered

**A. 포트에 `is_permitted(&self, check: impl Fn(...))` 유지.** 거부: 객체 안전하지 않음(`E0038`, 실증 확인) ⇒ `Arc<dyn ConsentManagerPort>` 컴파일 불가 ⇒ DI(ADR-001 §3) 무용. 포트의 핵심이 `dyn` 경계인데 무력화된다.

**B. boxed 클로저로 `is_permitted` 객체 안전화: `fn is_permitted(&self, check: Box<dyn Fn(&ConsentPermissions) -> bool>) -> bool`.** 객체 안전하나 거부: 모든 호출자가 클로저를 box 해야 하고 게이트 체크마다 힙 할당이 발생하는데 프로덕션 호출자는 **0**(6개 전부 테스트). `ConsentPermissions` 스냅샷 접근자가 더 단순·할당-가벼움·표현력 우월.

**C. 80 메서드 전부 전환하는 big-bang 단일 PR.** 거부: 3 포트 + ~80 impl + ~76 호출처를 한 PR 에 담는 XL diff 는 리뷰·bisect 불가이며, GDPR erase barrier 근처 단일 실수를 격리하기 어렵다. 슬라이스 계획은 각 diff 를 작게 유지하고 매 단계 erase-barrier 테스트를 green 유지.

**D. ADR-001 §2 균일성을 위해 `ConsentManagerPort` 를 async(`#[async_trait]`)로.** 거부: `ConsentManager` 는 async 표면이 없고(인메모리 `parking_lot::RwLock` + 동기 로컬 파일 I/O), ADR-021 이 async 외부 부수효과를 금지하며, async 는 41개 동기 게이트 체크에 무의미한 `.await` 를 강제한다. ADR-001 §2 의 의도는 I/O 포트이며, 순수 정책 권위체는 올바르게 동기다.

**E. `ConsentManager` 를 포트 뒤 어댑터 크레이트로 이동.** 거부: ADR-021(consent 상태는 의도적으로 `maekon-core` 에 유지되는 core 제품 정책)과 직접 충돌. 본 ADR 은 대신 core-local 포트를 추가한다.

## Known Follow-ups

1. **41개 구체 `Arc<ConsentManager>` 소비자를 `Arc<dyn ConsentManagerPort>` 로 마이그레이션** — 본 ADR 범위 밖(PR-1 은 포트만 도입); 포트 출하 후 별도 sweep 으로 추적.
2. **`spawn_blocking` 풀 사이징** — 수렴 후 스테이징에서 풀 saturation 이 보이면 전용 블로킹 풀 또는 `SqliteStorage` doc 에 이미 스케치된 read-only 두 번째 커넥션 경로 평가.
3. **PR-4..N 하위 트레잇 그룹핑** — 구현 시 어떤 하위 트레잇을 각 PR 로 묶을지 확정(목표 ≤ ~10 메서드/PR); 위 표는 구속적이지 않은 지침.

## Related Docs

- `docs/architecture/ADR-001-rust-client-architecture-patterns.md` — §2 async-trait, §3 DI, §6 의존성 방향, §7 포트 배치, §8 contract test
- `docs/architecture/ADR-007-async-runtime-safety-patterns.md` — async 런타임 비블로킹
- `docs/architecture/ADR-021-config-consent-core-placement.md` — consent 는 core 유지(경계 예외)
- `docs/architecture/ADR-024-conversation-content-guard-port.md` — 최근 포트-도입 선례
