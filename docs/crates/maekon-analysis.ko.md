[English](./maekon-analysis.md) | [한국어](./maekon-analysis.ko.md)

# maekon-analysis

독립적인 LLM 분석 파이프라인 크레이트 — 세그먼트 요약, 행동 레짐(regime)
분류, 벡터 RAG 검색, 적응형 클러스터링, 코칭, 그리고 (#7735 E-2 기준)
포커스/워크플로우 인텔리전스까지 담당한다. `maekon-app`의 무조건(항상 포함)
의존성이며, `hnsw` feature는 선택적 `hnsw_adapter` 모듈만 게이트한다.

## 역할

- **컨텍스트 조립 + 세그먼트화**: 원시 활동/프레임 텔레메트리를 LLM으로
  요약된, 시간 순으로 추적되는 세그먼트로 변환.
- **레짐 파이프라인**: 행동 레짐 생명주기(create, merge, split, mark_seen) +
  외부 소비를 위한 facade.
- **벡터 검색**: 임베딩 파이프라인(INT8 양자화), 적응형 검색 전략 선택
  (brute-force / IVF / IVF+binary / HNSW), 쿼리 확장, few-shot 예시 선택.
- **클러스터링**: k-means, GMM, HDBSCAN 탐지기를 공통 전략 트레이트 뒤에 배치.
- **작업 분류**: 규칙 기반 + LLM 정제 작업유형 분류, GUI 활동 집계,
  터미널/타이틀바 파싱.
- **튜닝 + 피드백**: 드리프트 탐지, 캘리브레이션 버퍼링, 피드백 추적,
  적응형 파라미터 해석.
- **다이제스트 + 인사이트**: 일간/주간 다이제스트 생성, 일간 인사이트,
  다이제스트 export.
- **코칭**: 능동적 생산성 코칭 엔진 + 템플릿 레지스트리.
- **포커스 + 워크플로우 인텔리전스** (#7735 E-2 신규 편입): 앱 사용
  relevance 스코어링, 워크플로우 세그먼트/플레이북 패턴 탐지, 그리고
  `FocusAnalyzer` 규칙 기반 제안 표면 (break / focus-time /
  restore-context / pattern-detected).

## 디렉토리 구조

```
maekon-analysis/src/
├── lib.rs                      # 크레이트 루트 — mod 선언 + 정제된 re-export
├── analyzer.rs                 # ContextAnalyzer — LLM 세그먼트 요약 + 레짐 분류
├── assembler.rs                # ContextAssembler — 분석 컨텍스트 조립 + PII 필터링
├── segment_buffer.rs, segment_summarizer.rs, llm_segment_summarizer.rs, content_tracker.rs
├── regime_classifier.rs, regime_detector.rs, regime_manager.rs,
│   regime_analysis_facade.rs, regime_goal_tracker.rs   # 행동 레짐 파이프라인
├── embedding_pipeline.rs       # EmbeddingPipeline — INT8 양자화
├── vector_retriever.rs, hybrid_search_service.rs, query_expander.rs, few_shot_selector.rs
├── adaptive_search/            # AdaptiveSearchCoordinator — 자동 전략 선택 (디렉토리 모듈)
├── hnsw_adapter.rs             # HnswAdapter — `#[cfg(feature = "hnsw")]`
├── kmeans_adapter.rs, gmm_detector/, hdbscan_detector.rs, clustering_strategy.rs
├── work_type_classifier.rs, llm_work_type_refiner.rs, gui_work_type_refiner.rs,
│   gui_aggregator.rs, terminal_detector.rs, title_bar_parser/, document_heading.rs
├── auto_tuner.rs                # EmaStatsTracker + DriftDetector
├── adaptive_trigger/, calibration_buffer.rs, feedback_tracker.rs,
│   param_resolver.rs, constraint_builder.rs
├── daily_digest_generator.rs, weekly_digest_generator.rs,
│   daily_insight_generator.rs, digest_exporter.rs
├── coaching_engine/            # CoachingEngine — guards + triggers (디렉토리 모듈)
├── coaching_template/          # 템플릿 레지스트리 + 내장 템플릿 (디렉토리 모듈)
├── pattern_miner/               # 순차/itemset GUI 패턴 마이닝 (디렉토리 모듈)
├── focus_analyzer/              # FocusAnalyzer — 세션 추적 + 규칙 제안 (#7735 E-2 신규 편입)
│   ├── mod.rs                  # FocusAnalyzer 구조체 + on_app_switch_with_context/analyze_periodic/on_idle_resume
│   ├── models.rs                # focus_shared 로부터 FocusAnalyzerConfig/CooldownType/SessionTracker/SuggestionCooldowns re-export
│   └── suggestions.rs           # break / focus-time / restore-context / pattern-detected 규칙 제안
├── workflow_intelligence.rs     # WorkflowIntelligence — 앱 사용 relevance 스코어링 + 플레이북 탐지 (#7735 E-2 신규 편입)
├── focus_shared.rs              # 공유 FocusAnalyzer 설정/쿨다운/트래커 타입 + make_rule_suggestion
├── suggestion_filter.rs, prompts.rs, fallback_analysis_provider.rs
├── belief_revision.rs           # ADR-023 Phase-2 로컬 LLM belief revision
└── error.rs                     # AnalysisError (ADR-019 typed code)
```

## FocusAnalyzer / WorkflowIntelligence (#7735 E-2)

`focus_analyzer/`와 `workflow_intelligence.rs`는 `src-tauri` composition
root에서 이곳으로 이동했다(#7735 추출 E-2) — 둘 다 이미 tauri-free,
ports-only 도메인 로직이었다. `FocusAnalyzer`는
`maekon_core::ports::focus_storage::FocusStorage`와
`maekon_core::ports::notifier::DesktopNotifier`에만 의존하며, `src-tauri`가
구체 어댑터(`SqliteStorage`, `TauriNotifier`/`LogOnlyNotifier`)를
와이어링하고 `maekon_analysis::focus_analyzer::FocusAnalyzer`와
`maekon_core::ports::focus_storage::FocusStorage`를 직접 소비한다
(`maekon_app`발 호환 re-export 없음).

## 의존성

- `maekon-core`: 모델, 포트, 에러 (유일한 normal-dependency 엣지)
- `thiserror`: `AnalysisError`
- `async-trait`: 포트 트레이트 구현 (예: 테스트의 `DesktopNotifier`)
- `chrono`, `tokio` (`sync` feature), `tracing`, `lru`, `sha2`, `serde`/`serde_json`
- `hdbscan` (기본 feature), `usearch` (선택적, `hnsw` feature)
- Dev 전용: `maekon-storage` (실 `SqliteStorage` 대상 cross-layer 회귀
  테스트), `tempfile`, `criterion`, `rand`, `maekon-core` `test-support`
  feature (`FakePiiSanitizer`)

## 빌드 / 테스트

```bash
cargo check -p maekon-analysis
cargo test -p maekon-analysis
cargo test -p maekon-analysis --features hnsw
```
