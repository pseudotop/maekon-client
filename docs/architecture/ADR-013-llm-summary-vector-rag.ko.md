[English](./ADR-013-llm-summary-vector-rag.md) | [한국어](./ADR-013-llm-summary-vector-rag.ko.md)

# ADR-013: LLM 세그먼트 요약 + Vector RAG

| 필드 | 값 |
|------|---|
| 상태 | Accepted |
| 날짜 | 2026-03-18 |
| 범위 | LlmSegmentSummarizer, EmbeddingProvider, VectorStore, EmbeddingPipeline, VectorRetriever, SemanticSearch, WeeklyDigest |

## 컨텍스트

적응형 계층 메모리(ADR-012)는 데스크톱 활동을 세그먼트화하고 규칙 기반 통계를 생성한다. 인텔리전스 사이클을 완성하려면 세그먼트에 LLM 생성 자연어 요약과 시맨틱 검색을 위한 vector 임베딩이 필요하다. 이를 통해 LLM 분석 파이프라인이 제안을 생성할 때 관련 이력 컨텍스트를 참조할 수 있고, 사용자가 의미를 기반으로 활동 이력을 검색할 수 있다.

## 결정 사항

### §1 두 단계 세그먼트 처리

세그먼트 종료 시:
- **Phase 1 (즉시)**: 규칙 기반 통계 저장, ContentActivity 레이블을 임베딩하여 vector로 저장. Monitor 루프 차단 없음.
- **Phase 2 (비동기)**: `AnalysisProvider::summarize_text()`를 통해 LLM 요약 생성, `activity_segments`에 저장 후 추가 vector로 임베딩.

Graceful 저하: LLM 또는 임베딩 실패가 세그먼트 저장을 방해하지 않음. 세그먼트는 항상 최소한 규칙 기반 통계와 함께 영구 저장된다.

### §2 AnalysisProvider 확장

기존 `AnalysisProvider` port trait에 `summarize_text()` 기본 메서드 추가. `Vec<Suggestion>` 대신 plain `String` 반환. 기본 구현은 `analyze()`를 호출하고 첫 번째 결과의 컨텐츠를 추출. 어댑터는 더 효율적인 단일 completion 호출로 오버라이드 가능.

### §3 새 `maekon-embedding` 크레이트

`fastembed-rs`(ONNX Runtime 래퍼)는 무거운 의존성(~30 MB dylib). `maekon-network`에서 격리하기 위해:
- 새 크레이트 `maekon-embedding`은 `maekon-core`에만 의존
- `fastembed-rs`를 사용하는 `LocalEmbeddingProvider` 포함
- `src-tauri`에서 feature-gated: `embedding = ["dep:maekon-embedding"]`
- `fastembed::TextEmbedding::embed()`는 동기 — `tokio::task::spawn_blocking`으로 래핑

워크스페이스가 11개에서 12개 크레이트로 성장.

### §4 EmbeddingProvider Port (비동기)

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError>;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError>;
    fn dimensions(&self) -> usize;
    fn model_id(&self) -> &str;
}
```

두 어댑터: `LocalEmbeddingProvider`(fastembed-rs, `maekon-embedding`에 위치)와 `RemoteEmbeddingProvider`(OpenAI API, `maekon-network`에 위치).

### §5 sqlite-vec 폴백이 있는 VectorStore Port

```rust
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn store(&self, vector: Vec<f32>, metadata: EmbeddingMetadata) -> Result<(), CoreError>;
    async fn search(&self, query: &[f32], limit: usize, time_decay_hours: f32) -> Result<Vec<SearchResult>, CoreError>;
    async fn enforce_retention(&self, max_days: u32) -> Result<u64, CoreError>;
    async fn mark_stale(&self, old_model_id: &str) -> Result<u64, CoreError>;
}
```

SQLite 구현:
- 기본: KNN 검색을 위한 `sqlite-vec` 확장(`vec0` 가상 테이블)
- 폴백: sqlite-vec를 사용할 수 없을 때 BLOB vector에 대한 Rust에서의 brute-force 코사인 유사도
- `SqliteVectorStore.use_vec_extension: bool`은 초기화 시 감지됨

Rowid 동기화: `embedding_vectors` 메타데이터 테이블과 `embedding_index` 가상 테이블이 동일 트랜잭션 내에서 rowid로 연결됨.

### §6 시간 감쇠 검색

결합 점수: `similarity × exp(-age_hours / decay_hours)`. 기본 감쇠: 168시간(1주 반감기). KNN에서 3배 후보를 over-fetch하고, 시간 감쇠로 재순위화하여 top-k 반환.

### §7 임베딩 버저닝

각 vector 행은 `model_id`를 저장. 모델 변경 시: stale 표시 → `original_text`에서 백그라운드 재임베딩(100 vector/사이클, ~500ms). Stale vector는 재임베딩될 때까지 검색 가능.

### §8 주간 다이제스트

세그먼트 데이터의 주간 롤업: 레짐/카테고리 분류, 상위 컨텐츠, 심층 작업 시간, 컨텍스트 전환, 이전 주와의 비교. `summarize_text()`를 통한 선택적 LLM 내러티브. 일요일 자정 또는 요청 시 생성.

### §9 PII 필터링

모든 텍스트는 임베딩 **전에** PII 필터링된다. `maekon-vision`에서 주입된 동일한 `PiiFilter` 클로저를 사용. 임베딩은 필터링된 텍스트의 시맨틱만 인코딩.

### §10 프라이버시: 행동 데이터로서의 임베딩 Vector

임베딩 vector는 사용자 활동의 시맨틱 패턴을 인코딩한다. PII 필터링 후에도 vector가 다음을 노출할 수 있다:
- 사용자가 작업하는 프로젝트/파일
- 작업 타이밍 패턴 (심층 작업 세션 발생 시기)
- 컨텍스트 전환 동작

완화 조치:
- 모든 텍스트는 임베딩 전에 PII 필터링됨(§9), `content_label` 메타데이터 포함
- Vector는 서버 동기화가 활성화된 경우를 제외하고 로컬에만 저장됨
- `activity_pattern_learning` 권한을 통한 동의 게이팅(GDPR Tier 4)
- 보존 정책(기본 90일)으로 이력 노출 제한
- 활동 세그먼트와 주간 다이제스트도 보존 적용(90일 / 52주)
- 사용자는 설정 초기화 또는 동의 철회를 통해 모든 vector를 삭제 가능

## 결과

- 워크스페이스가 11개에서 12개 크레이트로 성장 (`maekon-embedding`)
- `fastembed` + ONNX Runtime이 ~30 MB 외부 의존성 추가 (다운로드, 번들 아님)
- `sqlite-vec` 확장이 ~1 MB 추가 (선택 사항, brute-force 폴백 포함)
- `AnalysisProvider` trait이 `summarize_text()` 메서드 획득 (하위 호환 기본 구현)
- V10 마이그레이션이 `embedding_vectors`, `embedding_index`, `weekly_digests` 테이블 추가
- ContextAssembler가 RAG 강화 LLM 컨텍스트를 위한 `relevant_history` 파라미터 획득
- 두 가지 새 API 엔드포인트: 시맨틱 검색 + 주간 다이제스트

## 참조

- ADR-011: 독립형 분석 파이프라인
- ADR-012: 적응형 계층 메모리
- 설계 사양: 내부 LLM 요약/vector RAG 설계 노트
- 연구: fastembed-rs, sqlite-vec, EWMA 시간 감쇠
