[English](./ADR-016-config-change-bus.md) | [한국어](./ADR-016-config-change-bus.ko.md)

# ADR-016: 설정 변경 브로드캐스트 버스 (Config Change Bus)

**상태**: Accepted
**날짜**: 2026-04-17
**범위**: `oneshim-core::config_manager::ConfigManager`, 런타임에서 `AppConfig`를 읽는 모든 소비자

---

## 컨텍스트

이전까지 `ConfigManager`(`oneshim-core`)는 `Arc<RwLock<AppConfig>>`를 보유하고
`get()`을 통한 폴링 읽기만 노출했다. 사용자의 설정 변경에 반응해야 하는
모든 소비자가 자기 스냅샷을 캐시하고 각자 주기로 다시 읽어서 diff 했다.
`src-tauri/src/scheduler/loops/`의 스케줄러 루프들은 더티 체크 패턴을 제각기
구현했고, 일부 소비자(`oneshim-vision::privacy`,
`oneshim-analysis::regime_manager`)는 init 시점에 섹션을 캐시하여 이후 변경을
전혀 관찰하지 못했다. 설정 UI 토글이 각 소비자에 반영되기까지 1–30초가 걸렸다.

전체 인벤토리와 이 결정을 만든 feature-gap 분석은 내부 implementation record 로 보관합니다.

이 결합은 텔레메트리 익스포터(X2) 작업의 발목을 잡기도 했다. OTel 레이어
라이프사이클은 런타임 `telemetry.enabled` 변경에 맞춰 스왑되어야 하는데,
`main.rs`에서 1초마다 폴링하는 구조는 수용하기 어려웠다.

## 결정

`ConfigManager`는 이제 비공개 `Arc<Inner>` 내부에 `tokio::sync::watch::Sender<Arc<AppConfig>>`를
보유하며, 동시 쓰기를 직렬화하는 `parking_lot::Mutex<()>` writer-lock을 함께 가진다.
공개 API에는 메서드 두 개가 추가된다:

```rust
/// 전체 config 변경 통지를 구독한다.
pub fn subscribe(&self) -> watch::Receiver<Arc<AppConfig>>;

/// 구독자 등록 없이 값비싸지 않은 Arc 읽기.
pub fn snapshot(&self) -> Arc<AppConfig>;
```

기존의 `get()` / `update()` / `update_with()` / `reload()` 호출 지점은 **변경 없이**
그대로 작동한다. `ConfigManager: Clone`도 유지된다 (clone은 `Arc<Inner>`를 공유).
`src-tauri`, `oneshim-web`, 스케줄러 루프에 걸친 20개 이상의 기존 호출 지점이
수정 없이 호환된다.

## 효과

### 긍정적

- **변경 즉시 기상**: 구독자는 비동기 한 틱 내에 반응한다. "토글 반영까지 30초"
  문제는 `subscribe()`로 이주한 모든 소비자에서 사라진다.
- **소비자별 폴링 보일러플레이트 제거**: `select!`의 arm 하나로 해결되며,
  `subscribe()` 독스트링에 패턴을 명시한다.
- **리더가 라이터를 블록하지 않음**: `watch::Sender::borrow()`와 writer-lock은
  독립적인 동기화 원시를 사용한다.
- **추가형(additive) API**: 기존 호출 지점의 마이그레이션 비용이 0. 새 소비자만
  원할 때 전환.

### 부정적 — 감사(audit) 합쳐짐 위험

`tokio::sync::watch`는 **최신값 우선(latest-wins)** 시맨틱을 가진다. 두 번의
`changed().await` 기상 사이에 `A→B→A` 연속 업데이트가 발생하면, 구독자는 중간
상태 `B`를 관찰하지 못하고 최종 `A`만 본다.

**모든 중간 전이를 관찰해야 올바른 소비자는 기존 tick 기반 poll-and-diff
구조를 유지하거나, 별도 채널로 뮤테이션마다 시그널을 발행해야 한다.** 구체적
예로 `src-tauri/src/scheduler/loops/helpers.rs::audit_consent_and_pii_changes`는
`PiiFilterLevel` 전이마다 규정 준수 감사 로그를 남긴다. Phase 2에서 이 호출
지점은 **의도적으로 이주하지 않는다**. 섣부른 `subscribe-and-diff` 재작성은
사용자의 빠른 토글 시 감사 이벤트를 소리 없이 누락시킨다.

향후 단계에서 `subscribe()`로 이주하는 모든 소비자는 다음 리뷰 질문을 통과해야
한다: *"두 기상 사이에 A→B→A가 발생했을 때, 구독자가 B를 한 번도 보지 못해도
괜찮은가?"*. "아니오"라면 해당 소비자는 tick 기반 패턴을 유지하거나
`broadcast` 채널을 선택한다.

### 중립

- 그냥 "지금 값이 필요하다"인 소비자는 `subscribe()` 대신 `snapshot()`으로 충분.
  async도 diff도 필요 없다.
- `ConfigManager::get()`은 이제 `snapshot()` 위에서 구현되며, 의미와 비용은
  이전과 동일하다.

## 고려된 대안

- **`tokio::sync::broadcast`** — 기각. latest-wins가 맞는 상황에서 `Lagged` 처리와
  구독자별 큐 사이징을 강제해 복잡도만 증가.
- **섹션별 watch 채널** (`AppConfig` 최상위 16개 섹션 각각) — 기각. API 폭발.
  소비자에서의 diff는 저렴하다.
- **`arc_swap::ArcSwap<AppConfig>` + 폴링** — 기각. 락 경합은 줄지만 기상
  시그널이 없어 소비자가 여전히 폴링해야 한다.
- **`Clone`에서 `panic!`** (모든 호출 지점에 `Arc<ConfigManager>` 래핑 강제) —
  플래닝 단계에서 명시적으로 기각. 20개 이상 기존 호출 지점이 런타임에
  크래시될 것. `Arc<Inner>` 방식은 `Clone`을 저렴하게 유지한다.

## 참고

- 구현 기록: 내부 config telemetry 명세, 계획, feature-gap 분석 노트
- ADR-001: Rust 클라이언트 아키텍처 패턴 (Hexagonal 경계 준수)
- ADR-007: Async 런타임 안전성 패턴 (`parking_lot::Mutex`는 `.await`를 가로질러 유지하지 않음)
