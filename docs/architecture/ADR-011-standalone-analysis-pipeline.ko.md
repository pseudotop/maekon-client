[English](./ADR-011-standalone-analysis-pipeline.md) | [한국어](./ADR-011-standalone-analysis-pipeline.ko.md)

# ADR-011: 독립형 분석 파이프라인

| 필드 | 값 |
|------|---|
| 상태 | Accepted |
| 날짜 | 2026-03-18 |
| 범위 | 새 `maekon-analysis` 크레이트, `AnalysisProvider` port, 스케줄러 통합, Suggestion 모델 통합 |

## 컨텍스트

클라이언트는 풍부한 데스크톱 활동 데이터(앱 전환, OCR 텍스트, 윈도우 제목, 포커스 메트릭)를 수집하여 SQLite에 저장한다. 현재 제안(suggestion)은 규칙 기반(FocusAnalyzer)이거나 서버 의존적(SSE)이다. 수집된 컨텍스트를 LLM에 전달하고 연결된 서버 없이도 검토 가능한 다음 작업 후보를 생성하는 독립형 분석 사이클이 필요하다. 동일한 로직은 나중에 서버의 AI Intelligence 도메인에도 이식 가능해야 한다.

이 ADR은 기존 ADR에서 다루지 않는 다섯 가지 아키텍처 결정을 다룬다.

1. 새 어댑터 크레이트 생성
2. 분석 전용 port 계약
3. 멀티 port 소비자를 위한 오케스트레이터 패턴
4. 스케줄러 루프 확장
5. Suggestion 모델 통합

## 결정 사항

### §1 새 어댑터 크레이트: `maekon-analysis`

새 워크스페이스 멤버 `maekon-analysis`가 `crates/` 아래에 생성된다.

**의존성 규칙** (ADR-001 §6 확장):
```
maekon-core  ←  maekon-analysis  (새로운)
              ←  maekon-monitor
              ←  maekon-vision
              ←  ...
maekon-analysis  ←  src-tauri      (바이너리가 소비)
```

- `maekon-core`(port trait + domain 모델)에만 의존해야 한다.
- 다른 어댑터 크레이트에 의존해서는 안 된다(`maekon-network`, `maekon-storage` 등 없음).
- `src-tauri`는 DI를 통해 concrete 어댑터를 `maekon-analysis`에 연결한다.
- 오류 타입은 `thiserror` 사용(라이브러리 크레이트, ADR-001 §1 기준).
- 테스트는 ADR-001 §5 따름: `#[cfg(test)]` 모듈에서 수동 mock.

**네이밍 규칙**: `maekon-{domain}` (domain은 크레이트의 목적을 설명하는 단일 단어).

**크레이트 구조** (파일이 500줄을 초과하면 ADR-003 따름):
```
crates/maekon-analysis/
├── Cargo.toml
├── src/
│   ├── lib.rs              # pub re-exports
│   ├── analyzer.rs          # ContextAnalyzer 오케스트레이터
│   ├── pattern_miner.rs     # 순수 알고리즘 패턴 감지
│   ├── assembler.rs         # 컨텍스트 조립 + PII 필터링
│   └── prompts.rs           # 시스템 프롬프트 템플릿
```

### §2 AnalysisProvider Port 계약

새 port trait `AnalysisProvider`가 `maekon-core/src/ports/analysis_provider.rs`에 정의된다.

```rust
#[async_trait]
pub trait AnalysisProvider: Send + Sync {
    async fn analyze(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError>;

    fn provider_name(&self) -> &str;
}
```

**설계 근거**:
- 기존 `LlmProvider` trait과 분리(UI 자동화를 위한 `InterpretedAction` 반환).
- `Vec<Suggestion>`을 직접 반환 — LLM 응답 파싱은 오케스트레이터가 아닌 어댑터의 책임. 중간 타입(`SuggestionCandidate`)은 어댑터 내부에 private으로 유지.
- raw `context_json` + `system_prompt` 문자열 수락 — 오케스트레이터가 프롬프트 구성을 제어하고 어댑터는 HTTP 트랜스포트를 처리함.

**오류 매핑**: LLM 특정 실패(잘못된 응답, content filter, token limit)는 [ADR-019](./ADR-019-error-code-infrastructure.md)에 따라 `CoreError::Analysis { code: ProviderCode::AnalysisFailed, message }` (wire: `provider.analysis_failed`)를 사용. ADR-019 이전에는 시그니처가 `CoreError::Analysis(String)`이었음.

**구현**: `maekon-network/src/analysis_client.rs`에 위치하며 `RemoteLlmProvider`와 동일한 HTTP 클라이언트 인프라를 재사용. 동일한 struct에서 `LlmProvider`와 `AnalysisProvider` 모두 구현 가능.

**계약 테스트** (ADR-001 §5 기준):
```rust
#[cfg(test)]
mod tests {
    struct MockAnalysisProvider { ... }

    #[async_trait]
    impl AnalysisProvider for MockAnalysisProvider {
        async fn analyze(&self, context: &str, prompt: &str)
            -> Result<Vec<Suggestion>, CoreError> { ... }
        fn provider_name(&self) -> &str { "mock" }
    }
}
```

### §3 오케스트레이터 패턴

`ContextAnalyzer`는 `maekon-analysis`의 **concrete struct**이지 port trait이 아니다. 분석 사이클을 오케스트레이션하기 위해 여러 port를 소비한다.

**port로 만들지 않는 이유?**
- Port는 단일 책임 I/O 경계를 나타낸다(ADR-001 §7).
- 내부적으로 `StorageService`, `PatternMiner`, `ContextAssembler`, `AnalysisProvider`를 호출하는 오케스트레이터는 여러 책임을 가진다.
- port로 만들면 모든 소비자가 전체 오케스트레이션 표면을 mock해야 한다.

**패턴**:
```rust
pub struct ContextAnalyzer {
    storage: Arc<dyn StorageService>,
    analysis_provider: Arc<dyn AnalysisProvider>,
    pattern_miner: PatternMiner,      // 소유됨, 순수 알고리즘
    context_assembler: ContextAssembler, // 소유됨, 순수 빌더
    config: AnalysisConfig,
    last_analysis_at: Mutex<Option<DateTime<Utc>>>,
}
```

- Port 의존성은 생성자를 통해 주입(Arc<dyn T>, ADR-001 §3 기준).
- 순수 알고리즘 컴포넌트(`PatternMiner`, `ContextAssembler`)는 직접 소유 — 외부 I/O가 없고 port 추상화가 필요 없음.
- 내부 가변성(`Mutex`)은 throttle 상태 추적에만 사용(ADR-001 §2 기준).
- `src-tauri/src/agent_runtime_support.rs`에서 다른 DI wiring과 함께 생성됨.

**선례**: `maekon-automation`의 `AutomationController`가 동일한 패턴을 따름 — 여러 port를 소비하는 concrete struct.

### §4 스케줄러 루프 확장

새 분석 루프가 10번째 백그라운드 루프로 스케줄러에 추가된다.

**통합 포인트**: `src-tauri/src/scheduler/loops.rs` — 새 `spawn_analysis_loop()` 메서드.

**루프 구조** (기존 `spawn_focus_loop`, `spawn_sync_loop` 패턴 따름):
```rust
pub(super) fn spawn_analysis_loop(
    &self,
    config: AnalysisConfig,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let analyzer = self.context_analyzer.clone();
    let storage = self.storage.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(
            Duration::from_secs(config.interval_secs)
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match analyzer.analyze().await {
                        Ok(suggestions) => { /* store + notify */ }
                        Err(e) => warn!("analysis failure: {e}"),
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("analysis loop ended");
                    break;
                }
            }
        }
    })
}
```

**네이밍 규칙**: `spawn_{name}_loop()`, 기존 패턴 일치.

**이벤트 구동 경로**: `FocusAnalyzer.on_app_switch_with_context()`와 함께 `spawn_monitor_loop`에 연결. 앱 전환 시 두 가지가 병렬로 실행되며 모두 `Suggestion`을 출력함.

**중복 제거**: FocusAnalyzer(규칙)와 ContextAnalyzer(LLM) 모두 동일 이벤트에 대해 제안을 생성하면 LLM 기반이 우선(더 높은 정보 밀도). 저장 전에 스케줄러가 중복 제거.

### §5 Suggestion 모델 통합

`LocalSuggestion`은 폐기된다. 모든 제안은 `maekon-core/src/models/suggestion.rs`의 통합 `Suggestion` 모델을 사용한다.

**새 필드**: 서버 SSE 역직렬화와의 하위 호환을 위해 `#[serde(default)]`가 있는 `source: SuggestionSource`.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SuggestionSource {
    #[default]
    RuleBased,
    LlmLocal,
    LlmServer,
}
```

**마이그레이션**: `FocusAnalyzer`가 `LocalSuggestion` 대신 `Suggestion`을 출력. 기존 `LocalSuggestion` enum 변형은 적절한 `suggestion_type`과 `content`를 가진 `Suggestion`으로 매핑.

**SQLite 스키마**: V8 마이그레이션이 `local_suggestions`를 교체하는 통합 `suggestions` 테이블을 생성.

**공존 규칙**: 서버가 활성 상태이고 SSE를 통해 제안을 반환하면 로컬 LLM 분석(`LlmLocal`)이 억제된다. 규칙 기반(`RuleBased`)은 항상 실행된다.

## 결과

- 워크스페이스가 10개에서 11개 크레이트로 성장.
- `maekon-analysis`는 격리된 상태에서 완전히 테스트 가능(`maekon-core` trait에만 의존).
- `PatternMiner`와 `ContextAssembler`는 서버로 이식 가능(클라이언트 특정 의존성 없음).
- 스케줄러가 9개에서 10개 루프로 성장.
- `LocalSuggestion` 제거; 모든 코드 경로가 `Suggestion` 사용.
- `maekon-network`의 `AnalysisProvider` 어댑터는 오케스트레이터 변경 없이 서버 측 DSPy 파이프라인으로 교체 가능.

## 참조

- ADR-001 §1-7: 오류 타입, async trait, DI, 크레이트 경계, port
- ADR-003: 디렉토리 모듈 패턴 (파일이 500줄을 초과하면 적용)
- ADR-009: 클라이언트 아키텍처 베이스라인 (런타임 조합, delivery 레이어)
- 설계 사양: 내부 독립형 분석 파이프라인 설계 노트
