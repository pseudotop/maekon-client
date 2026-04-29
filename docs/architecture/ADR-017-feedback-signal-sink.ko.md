[English](./ADR-017-feedback-signal-sink.md) | [한국어](./ADR-017-feedback-signal-sink.ko.md)

# ADR-017: FeedbackSignalSink

**상태 (Status)**: 채택됨 (Accepted)
**날짜 (Date)**: 2026-04-18
**범위 (Scope)**: `oneshim-core::ports::feedback_signal_sink`, `oneshim-suggestion::FeedbackSender`, `oneshim-analysis::CoachingEngine/RegimeClassifier`, `src-tauri::feedback_sink::CompositeFeedbackSink`

---

## 배경 (Context)

본 ADR 이전까지 `commands/suggestions.rs::handle_suggestion_action` 은 수락/거절/연기(accept/reject/defer) 를 `FeedbackSender::send_feedback` 로 라우팅 했고, 그 내부에서 `ApiClient` 를 통해 서버로 전송했다. 실패 시 `FeedbackRetryQueue` 에 적재되며 스케줄러가 이를 비운다. 그러나 클라이언트 내부의 어떤 구성요소도 이 이벤트를 수신하지 못했다 — `CoachingEngine` 은 제안이 수락됐다는 사실을 학습하지 못했고, `RegimeClassifier` 는 어떤 regime 의 제안이 수락/거절되는지 알 수 없었다.

2026-04-16 Feature Gap Analysis (X3, C1 잔여분) 참조.

## 결정 (Decision)

`oneshim-core::ports::feedback_signal_sink` 에 새 포트 `FeedbackSignalSink` 를 추가한다:

```rust
#[async_trait]
pub trait FeedbackSignalSink: Send + Sync {
    async fn record_user_reaction(
        &self,
        feedback: &SuggestionFeedback,
    ) -> Result<(), CoreError>;
}
```

`src-tauri/src/feedback_sink/mod.rs` 의 `CompositeFeedbackSink` 가 `Arc<CoachingEngine>` + `Arc<parking_lot::Mutex<RegimeClassifier>>` 로 팬아웃(fan-out)한다 — 각 소비자(consumer)는 `Option<>` 으로 감싼다.

`FeedbackSender` 는 `Option<Arc<dyn FeedbackSignalSink>>` 를 받는다. `send_feedback` 은 서버 호출 **전에** 싱크(sink)를 먼저 호출하므로, 서버가 연결 불가한 상황에서도 로컬 학습 경로는 그대로 적응된다. 기존 `FeedbackSender::new(api)` 는 `new_with_sink(api, None)` 으로 위임하는 심(shim) 으로 유지된다.

## 결과 (Consequences)

### 긍정 (Positive)

- CoachingEngine + RegimeClassifier 가 사용자 반응(user-reaction) 신호에 대한 안정된 채널을 갖는다. 실제 학습 알고리즘은 포트(port) 를 건드리지 않고 후속 페이즈에서 구현 가능하다.
- 팬아웃(fan-out) 은 구성 루트(composition-root) 에서만 조립되므로 크레이트 간 어댑터 의존성이 없다.

### 부정 / 제약 (Negative / Constraints)

- **지연 예산 (Latency budget)**: 구현체는 ~10ms 이내에 반환해야 한다. 블로킹 작업(DB 쓰기, 네트워크 호출, 무거운 계산) 은 구현체 내부에서 `tokio::spawn` 으로 오프로드해야 한다. `FeedbackSender::send_feedback` 은 사용자 경로(user-path) accept/reject 에서 싱크를 동기적으로 `await` 하므로, 이 예산을 깨면 의도적으로 분리했던 쓰기 경로 대기(write-path wait) 가 다시 들어온다.
- **Err 의미 (Err semantics)**: `Result<(), CoreError>` 의 `Err` 는 프로그래머 버그(mutex poisoning, invariant 위반) 전용이다. 예상 가능한 모든 실패(네트워크, DB, 일시적 unavailability) 는 구현체 내부에서 로그 후 삼켜야(log and swallow) 하며 `Err` 로 상위로 전파하면 안 된다. 호출부는 `Err` 를 `warn!` 로 기록하되, 사용자 경로 실패로는 간주하지 않는다.
- **재시도 순서 (Retry ordering)**: `FeedbackSender::send_feedback` 은 *매 호출*마다 API 호출 전에 싱크를 발화한다. 네트워크 장애로 서버 호출이 실패하면 스케줄러가 `FeedbackRetryQueue` 를 배수하며 `accept`/`reject` 를 재호출한다(`scheduler/loops/suggestions.rs`). 즉, 사용자 1 액션에 대해 N 번의 재시도가 있으면 싱크도 N 번 발화된다. Phase 3 에서는 이 해저드를 감수한다 — 현재 스텁(`CoachingEngine::record_user_reaction`, `RegimeClassifier::record_user_reaction`) 은 `debug!` 로그만 찍어 idempotent 하기 때문이다. **후속 학습 구현은 `suggestion_id` 기준 idempotent 여야 한다 — 구현 레이어에서 중복 제거(seen-set / last-seen timestamp) 하거나, 후속 페이즈에서 싱크 호출을 `send_feedback` 밖(`commands/suggestions.rs::handle_suggestion_action`) 으로 옮겨 네트워크 재시도와 무관하게 사용자 액션 당 1 회만 발화되도록 해야 한다.** 학습 알고리즘을 도입하는 addendum ADR 은 두 옵션 중 하나를 명시적으로 선택해야 한다.

### 중립 (Neutral)

- `FeedbackSender::new_with_sink(api, None)` 은 항상 유효하다 — 텔레메트리 꺼짐(telemetry-off)/테스트/coaching 비활성화 경로 모두 변경 없이 동작한다.

## 대안 검토 (Alternatives considered)

- `tokio::sync::broadcast` 이벤트 버스 — 기각(rejected). 런타임 태스크와 사이즈 설정 부담을 소비자 2개를 위해 추가하는 것은 과다하며, 이벤트 큐잉(per-event queuing) 이 요구되지 않는다.
- `FeedbackSender` 가 `Arc<CoachingEngine>` 을 직접 참조 — 기각. 헥사고날 경계 위반(`oneshim-suggestion` 이 `oneshim-analysis` 에 의존하게 된다).
- 소비자 당 포트 분리(`CoachingSink`, `RegimeSink`) — 기각. 둘 중 하나만 선택하는 호출자가 없어 포트 표면만 늘어나고, `CompositeFeedbackSink` 가 `Option<>` 로 소비자별 on/off 를 이미 처리한다.
- 싱크를 서버 호출 **이후** 실행 — 기각. 서버 실패 시 로컬 학습이 차단되며, 로컬 신호는 독립적 가치가 있다.

## 참조 (References)

- 구현 기록: 내부 regime feedback learning 명세와 feature-gap 분석 노트
- ADR-001 헥사고날 경계 (Hexagonal boundary)
- ADR-007 `parking_lot::Mutex` 는 `.await` 를 건너지 않는다 — `CompositeFeedbackSink` 가 준수 (락 획득 → 메서드 호출 → `.await` 이전에 락 드롭)
