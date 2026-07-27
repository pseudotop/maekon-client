[English](./ADR-023-local-symbolic-memory-graph.md) | [한국어](./ADR-023-local-symbolic-memory-graph.ko.md)

# ADR-023: Standalone 클라이언트용 로컬 심볼릭 메모리-그래프 레이어

**Status**: Accepted — 결정 비준 완료 및 **전체 구현 완료**. landed: substrate(AC #1·#2·#7·#9), Phase-1 LLM-free consumer slice(AC #3·#4 — D3 digest→claim 승격 + D5 rule taxonomy), rule-seeded `Associated` app-sequence 엣지(#4441 box 5), Phase-2 LLM-gated belief revision(AC #5·#6·#8 — D1 relation 엣지 + D2 contradiction→supersede, 로컬-LLM 게이트, egress-safety 감사 완료). on-demand markdown 렌더 배선(web export)은 보류(#4441 추적); 광범위한 기존 right-to-erasure 갭은 #4478 에서 별도 추적.
**Date**: 2026-05-30
**Scope**: `crates/maekon-analysis`, `crates/maekon-core/src/ports`, `crates/maekon-core/src/models`, `crates/maekon-storage/src/migration`, `crates/maekon-storage/src/sqlite`
**Related**: ADR-011 (Standalone Analysis Pipeline), ADR-012 (Adaptive Tiered Memory), ADR-013 (LLM Segment Summary + Vector RAG), ADR-018 (Regime Manager Persistence), ADR-001 (Rust Client Architecture Patterns)
**Implementation**: substrate — `crates/maekon-core/src/models/memory_graph.rs`, `crates/maekon-core/src/ports/memory_graph_port.rs`, `crates/maekon-storage/src/migration/v34_memory_graph.rs`, `crates/maekon-storage/src/sqlite/memory_graph_impl.rs`. Phase-1 consumer slice(D3/D5) — `crates/maekon-analysis/src/claim_promoter.rs`(순수 promoter), `daily_digest.rs`의 `DigestExporter::to_markdown_with_claims`, scheduler aggregation-loop 배선(`src-tauri/src/scheduler/loops/system.rs`). D1/D2 LLM-gated 엣지는 별도 추적.

---

## 배경 (Context)

Maekon 클라이언트는 **server 없이도(standalone) 좋은 UX**를 제공해야 한다. ADR-011은 이미 로컬 메모리 파이프라인이 설계상 server-optional 임을 확립했다: 로컬 임베딩(fastembed/ONNX `all-MiniLM-L6-v2` INT8, `default = ["fastembed-local"]`), IVF/HNSW 벡터 스토어, hybrid FTS5+vector Reciprocal-Rank-Fusion 검색, kmeans/gmm/hdbscan regime 탐지, pattern mining, segment summarization, digest/markdown export — 모두 네트워크 I/O 없이 클라이언트에서 동작한다.

server 측 `ontology_reasoning` / `knowledge_management` 도메인은 더 풍부한 *심볼릭* 메모리 모델(typed knowledge-graph 엣지, 모순 감지, 신념 수정, durable thesis/insight 노드)을 제공한다. **이 도메인들은 standalone 에 아무 이득이 없다**: 전부 server 측에만 존재하고 클라이언트 미러가 없으며 server 가 동작할 때만 사용된다. 이 ADR이 답하는 질문은 *standalone* 클라이언트가 기존 로컬 벡터/regime 메모리 위에 **로컬 심볼릭 지식그래프 / 메모리 레이어**(로컬 LLM-Wiki / second-brain 식: 로컬 스토어 + 로컬 LLM)를 가져야 하는가이다.

이 질문은 **Andrej Karpathy 의 LLM Wiki** 패턴 — 이 접근의 원본(canonical origin) — 에서 비롯한다: LLM 이 유지하는, 상호연결된 로컬 markdown 지식베이스로, 새 소스마다 읽어 entity/topic 페이지에 *통합*(모순 표기)하여 지식을 쿼리마다 재도출하지 않고 한 번 컴파일 후 최신 유지한다. 결정적으로 이는 **로컬·server-less** 지식그래프이며, 바로 여기서 다루는 standalone 모델과 정확히 일치한다.

`crates/` 실측 조사 결과 현재 로컬 메모리 레이어는 **벡터 + regime + flat 레코드뿐이며, 그래프/엣지 구조가 전혀 없다**:

- 핵심 메모리 단위 `SegmentSummary`(`maekon-core/src/models/tiered_memory/segment.rs`)는 다른 segment 와 연결되는 필드가 없다. `EmbeddingMetadata`/`SearchResult`는 순수 검색 payload 다. `Regime`은 feature 공간의 centroid 점이며 엣지를 가진 노드가 아니다.
- `RegimeManager`(`maekon-analysis/src/regime_manager.rs`, ADR-012)는 flat `Vec<Regime>`을 보유한다. `merge_two`는 weighted centroid 를 계산하고 **두 source regime 을 파괴적으로 제거**하며 provenance/parent/split 이 없다. (regime 소스에서 `split`/`provenance`/`merged_from`/`parent_regime` grep 결과 empty — 산문상의 "split" 언급은 미구현 aspirational.)
- 검색(`hybrid_search_service.rs`, `vector_retriever.rs`)은 similarity + FTS RRF 랭킹뿐 — traversal/typed-relation following 없음.
- `DigestHighlight`(`models/daily_digest.rs`: typed `{Achievement|Warning|Suggestion}` + optional 단일 `segment_id`)가 **유일한** claim-like 구조이자 유일한 soft cross-reference 지만, **매 실행마다 재생성되는 ephemeral LLM 출력**이며 durable 노드가 아니고 evidence 엣지/모순·신념수정이 없다.
- 스토리지(`maekon-storage`, `CURRENT_VERSION = 33`)는 구조적 containment FK 만 있고 **edge/relation/claim/thesis/contradiction/evidence/provenance 테이블이 없다**(grep 확인).

따라서 standalone 클라이언트가 로컬에 결여한 벤치마크 delta 는 모두 **부재**로 확인된다: **D1** typed epistemic 엣지(supports/refines/contradicts), **D2** 모순 감지 + 신념 수정, **D3** evidence 엣지를 가진 durable thesis/claim 노드(부분: `DigestHighlight`가 최근접 analog), **D4** near-miss/contrastive 엣지, **D5** 인지 메모리 분류(semantic/episodic/procedural/reflective).

결정을 제약하는, 정직하게 명시해야 할 두 사실:

1. **substrate 는 준비됐고 greenfield 다.** SQLite 가 이미 스토어이고 per-version 마이그레이션 프레임워크가 있다(`v31_regime_manager_state.rs` singleton-blob 패턴이 직접 선례). 안정적 노드 식별 primitive 가 **이미 존재한다**: `segment_id`가 `SegmentSummary`/`EmbeddingMetadata`/`DigestHighlight.segment_id` 전반의 보편 키라, 새 엣지가 즉시 실제 메모리 단위를 참조할 수 있다. `maekon-analysis`는 `maekon-core`에만 의존하므로 새 그래프 컴포넌트가 헥사고날 경계(ADR-001)를 위반하지 않는다. 새 노드는 기존 테이블로 vector + FTS recall 을 공짜로 얻는다.

2. *심볼릭* delta 의 추론 엔진은 **옵션인 로컬 LLM** 이다. 클라이언트에 심볼릭 reasoner 는 없다. typed-edge 추론(D1)과 모순 판정(D2)은 현실적으로 LLM 이 필요하다. 클라이언트엔 로컬-LLM 경로가 있다 — `AnalysisProvider` 포트는 `config.ai_provider.llm_api`를 로컬 Ollama 엔드포인트로 지정하면 100% 오프라인으로 충족되고(`AnalysisClient`가 `AiProviderType::Ollama`를 API 키 없이 특수 처리), `DailyInsightGenerator`는 이미 JSON 구조화 LLM 추출을 수행해 typed edge/claim 으로 일반화된다. **그러나 이 경로는 옵션이며 `llm_api` 미설정 시(기본 설치) `NoOpAnalysisProvider`로 degrade 한다**("No LLM provider configured"). 생성이 LLM 에 hard-depend 하는 delta 는 오프라인 사용자에게 silently 비게 되므로 이를 설계로 우회해야 한다.

결정은 server 패리티 추구에 대한 열정이 아니라 이 증거를 따라야 한다 — durable 그래프 substrate 는 싸고 오프라인에서 유용하며, LLM 의존 심볼릭 delta 는 가치 있으나 조건부다.

## 결정 (Decision)

**Option C (Hybrid-minimal)** 채택: durable 로컬 메모리-그래프 **substrate** 와 **LLM 없이** 동작하는 delta 를 도입하고, LLM 의존 delta 는 기존 옵션 로컬-LLM 경로 뒤에 명시적 graceful degradation 과 함께 게이트한다. 최저 가치 delta 는 defer 한다.

### 1. Durable 그래프 substrate (LLM 불필요)

`v31` 선례를 따라 SQLite 마이그레이션(`v34_memory_graph`, `CURRENT_VERSION`을 34로 상향)을 추가한다:

```sql
-- claim/thesis 노드 (durable; digest 재생성에도 생존)
CREATE TABLE memory_claims (
  claim_id     TEXT PRIMARY KEY,   -- generate_id("clm") per ADR-022
  kind         TEXT NOT NULL,      -- D5 taxonomy: semantic|episodic|procedural|reflective
  text         TEXT NOT NULL,
  source       TEXT NOT NULL,      -- e.g. "digest_highlight", "pattern_miner", "llm"
  confidence   REAL NOT NULL,
  status       TEXT NOT NULL,      -- active|superseded|retracted
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);

-- 메모리 단위 간 typed 엣지 (claim 및/또는 segment_id 키 단위)
CREATE TABLE memory_edges (
  edge_id      TEXT PRIMARY KEY,   -- generate_id("edg")
  src_id       TEXT NOT NULL,      -- claim_id or segment_id
  dst_id       TEXT NOT NULL,
  edge_type    TEXT NOT NULL,      -- evidence|supports|refines|contradicts (D1)
  confidence   REAL NOT NULL,
  evidence_ref TEXT,               -- optional segment_id/frame_id provenance
  source       TEXT NOT NULL,      -- "rule" | "llm"
  created_at   INTEGER NOT NULL
);
CREATE INDEX idx_memory_edges_src ON memory_edges(src_id, edge_type);
CREATE INDEX idx_memory_edges_dst ON memory_edges(dst_id, edge_type);
```

`maekon-core/src/ports/`에 `MemoryGraphPort` 트레이트(async, `&self`, ADR-001)를 정의하고 `SqliteStorage`(이미 10+ disjoint 포트 트레이트 집약)가 구현한다. 노드 ID 는 `generate_id`(ADR-022)에 새 prefix `clm`/`edg`를 사용한다(ADR-022 prefix 테이블에 등록).

**근거**: substrate 는 마이그레이션 1개 + 포트 1개이며 `segment_id`를 노드 식별자로 재사용하고 LLM 의존을 추가하지 않는다. 기존 `embedding_vectors`/FTS5 가 새 노드에 vector·lexical recall 을 별도 작업 없이 제공한다.

### 2. Phase 1 delta — durable, LLM-free (먼저 출시)

- **D3 durable claim 노드 + evidence 엣지**: daily-digest 콘텐츠를 `memory_claims` 행 + 소스 `segment_id`로의 `evidence` 타입 `memory_edges` 행으로 승격. **오프라인** 소스는 digest **timeline**(`TimelineEntry` — LLM 없이도 `DailyDigestGenerator`가 closed segment 로 항상 생성, `segment_id` 보장) → `episodic` claim + evidence 엣지; 이것이 매 설치에서 그래프를 비지 않게 한다. LLM insight 가 돌았으면 각 `DigestHighlight`도 추가 승격(kind 는 D5 rule, optional `segment_id` 있으면 evidence 엣지). 매 실행 재생성되던 digest 콘텐츠를 durable·queryable 노드로 전환. `DigestExporter::to_markdown_with_claims`가 누적 claim 을 LLM-Wiki 식 로컬 second-brain 으로 렌더 — 완전 오프라인, LLM 불필요. `crates/maekon-analysis/src/claim_promoter.rs`(순수 builder) + scheduler aggregation loop 에 구현.
- **D5 인지 분류**: `kind` 컬럼(`semantic|episodic|procedural|reflective`)을 claim 생성 시점의 rule 기반 태그로 구현. 구현된 규칙: timeline 유래 claim → `episodic`; LLM highlight → `Achievement`→`episodic`, `Warning`→`reflective`, `Suggestion`→`procedural`(`HighlightType` 전수). LLM 불필요.
- **Rule-seeded `Associated` 엣지 (#4441 box 5 — landed)**: 연속 timeline claim 을 rule-seeded `Associated` 엣지(`source = "rule"`, `src` = 이전 → `dst` = 이후)로 연결 — 프로덕션에서 항상 채워지는 digest timeline 순서로 app-**sequence** 신호를 실현. `SegmentSummary.patterns_detected`(scheduler 경로에서 비어 있음) 대신 timeline 을 사용하고, app-node 식별 문제를 회피한다(`EdgeType::Associated`는 기존 claim 노드를 연결, 합성 app 노드가 아님). app-name **co-occurrence** 엣지(별도 app 노드/스키마 변경 필요)는 out of scope.

### 3. Phase 2 delta — LLM-gated (로컬 LLM 설정 시에만 출시)

로컬 LLM 이 설정된 경우에만(`config.ai_provider.llm_api.is_some()`, 로컬 Ollama 포함):

- **D1 typed epistemic 엣지**(`supports`/`refines`/`contradicts`): `AnalysisProvider` 포트 뒤의 신규 relation-extraction 컴포넌트가 `DailyInsightGenerator`의 JSON 구조화 추출을 일반화해 생성.
- **D2 모순 감지**: claim 쌍에 대한 모순 패스(vector-recall 후보 → LLM 판정)가 `contradicts` 엣지를 emit. **신념 수정은 얕고 opt-in**: 모순된 claim 은 `status` `active → superseded`(또는 confidence decay)로 전이하고 provenance 와 함께 새 노드/엣지로 기록. `RegimeManager` merge(ADR-012/ADR-018)는 **건드리지 않는다**; 신념 수정은 `memory_claims`에만 작용하고 regime 에는 절대 작용하지 않는다.

**Degradation 계약**: LLM 미설정 시 Phase-2 컴포넌트는 기존 LLM 단계(suggestion, segment narrative, daily insight)와 동일하게 `FallbackAnalysisProvider → NoOpAnalysisProvider`로 degrade 한다. 그래프는 **결코 silently 비지 않는다** — Phase-1 substrate(D3+D5)가 rule 기반 경로로 항상 채워지기 때문. 이것이 C 를 B 와 구분하는 핵심 correctness 속성이다.

### 4. 명시적 defer

- **D4 near-miss/contrastive 엣지**: defer. 노력 대비 standalone-UX 가치 최저, seed 신호 없음. Phase 1+2 가 안착하고 사용자가 contrastive recall 을 요구할 때 재검토.
- **regime 신념 수정**: 명시적 out-of-scope. ADR-012 의 파괴적 centroid merge 는 그대로; 이 ADR 은 regime 에 provenance/split 을 추가하지 않는다.
- **Traversal-first 검색**: Phase 1/2 는 RRF hybrid 검색을 주 retriever 로 유지; 엣지는 augment(re-rank/expand)할 뿐 `hybrid_search_service`를 대체하지 않는다.

### 5. 범위 명확화 — claim 을 제안/코칭 입력으로 사용 (defer; 2026-07, #8058 P2-2)

2026-07 기능 감사에서 `coaching_engine`·suggestion 크레이트가 `MemoryGraphPort`
참조 **0건**임이 관찰됨: 그래프의 read 표면은 (a) 웹 대시보드
(`handlers/memory_claims.rs` — list/retract, 즉 표시 + 사용자 액션), (b)
`belief_revision`(기본 **off**) 뿐. 위에서 언급한 검색-augment read-path(엣지
re-rank/expand)조차 **아직 미배선** — `semantic_search_service` 는 claim/edge 를
읽지 않는다. 요컨대 active claim/edge 는 현재 **표시 + belief revision 전용**으로만
축적되며, 생성(generation) 결정에 흐르지 않는다.

이는 비준된 범위에 **부합**하며 회귀가 아니다. 수용 기준(아래)은 substrate·승격·
durability·belief revision·degradation·markdown 렌더를 다루며, claim 이 제안/코칭
*생성*에 반영된다는 항목은 **없다**. 표시 외 ADR 의도 read-path 는 검색 augment
(엣지가 hybrid 검색을 re-rank/expand)이지, claim 텍스트를 제안/코칭 프롬프트에
주입하는 것이 **아니다**. 둘은 실질적으로 다른 설계(검색 re-rank vs 프롬프트-컨텍스트
주입 vs 코칭 게이트 신호)이며, 각각 고유의 프라이버시 표면(claim 텍스트는
사용자-파생이며 생성 경계를 새로 넘게 됨)·egress 감사·평가 요건을 가진다.

**결정**: claim-as-generation-input consumer 는 **신규·미비준 설계**이므로 임시로
도입하지 않고 전용 follow-up(ADR amendment 또는 신규 ADR)으로 연기한다. 이미
범위화된 검색 augment(위 항목) 배선이 더 작고 ADR-정합적인 첫 단계이며 마찬가지로
follow-up 으로 추적한다. 이 명확화에는 런타임 변경이 수반되지 않는다 —
"display-only" 관찰이 구현 버그로 오인되지 않도록 경계를 기록할 뿐이다.

## 결과 (Consequences)

### 긍정

- durable·revisable **오프라인** 지식 레이어(D3+D5)가 기본 설치에서 **LLM 의존 없이** 존재하고, 기존 `DigestExporter`로 markdown 렌더 — 단순 검색이 아닌 진짜 standalone second-brain.
- substrate(테이블 + 포트)는 검증된 선례(`v31`) 위 v34 마이그레이션 1개이며 `segment_id` 노드 식별자를 재사용하고 ADR-001 헥사고날 경계(`maekon-analysis → maekon-core`만)를 준수.
- D1/D2 레이어가 로컬 LLM(Ollama) 존재 시 **동일** substrate + **동일** degradable `AnalysisProvider` 포트 위에 얹힘 — 오프라인 full neural-grade 그래프, misleading 빈 그래프 대신 정직한 NoOp 폴백.
- ADR-012/ADR-018 regime 생애주기 변경 없음; Option B 보다 blast radius 작음.

### 부정

- 2단계 전달: 가장 풍부한 delta(D1 typed 엣지, D2 모순)는 기본 no-LLM 설치에 부재. non-empty Phase-1 baseline + 명확한 게이팅으로 완화.
- 신규 `memory_claims`/`memory_edges` 테이블이 Option A(무작위) 대비 마이그레이션·유지 surface 추가.
- Phase-1 신념 수정은 의도적으로 얕음(supersede / confidence decay), true multi-hypothesis revision 아님.
- D4 미해결; 벤치마크 delta 1개 open.

### 중립

- server `ontology_reasoning`/`knowledge_management` 도메인은 orthogonal; 이 레이어는 그에 의존하지도 이득을 보지도 않으며 순수 로컬이다.
- 신규 ID prefix `clm`/`edg`를 ADR-022 prefix 레지스트리에 등록해야 함.
- 첫 실행 ONNX weight 다운로드("로컬" 임베딩의 유일한 네트워크 접촉)는 불변; 그래프 레이어 자체는 네트워크 I/O 추가 없음.

## 고려한 대안 (Alternatives Considered)

**A. 벡터/regime 전용 메모리 유지(무작위).** 장기 답으로는 기각. 오늘의 오프라인 UX(semantic recall + digest)는 유용하나 검색에서 멈춘다 — durable·revisable 지식 누적 불가, 유일 패리티 경로(server 도메인)는 standalone 이득 0. Phase 1 이 원치 않는 것으로 판명되면 fallback 으로 허용 가능하나 second-brain UX 를 원천 차단.

**B. server 패리티 full 로컬 심볼릭 그래프(D1–D5 전체, traversal 검색, regime merge 를 신념 수정으로 대체).** gold-plating 으로 기각. load-bearing primitive(typed-edge 추론, 모순 판정, 신념 수정)가 전부 **옵션** 로컬 LLM 에 hard-depend → 기본 no-LLM 설치에서 정작 타겟인 오프라인 사용자에게 그래프 대부분이 silently 빈다. regime merge 를 신념 수정으로 재작성하면 ADR-012/ADR-018 대비 마이그레이션/provenance 리스크가 marginal 이득에 비해 크고, 특히 D4 는 노력/가치 비율이 나쁘다. 조건부 payoff 대비 net-new surface 과다.

**C. Hybrid-minimal (채택).** substrate 를 한 번 구축하고 LLM-free delta(D3+D5)를 먼저 출시해 모든 설치에서 standalone UX 가 개선되며, LLM 의존 delta(D1/D2)는 기존 degradable 로컬-LLM 경로 뒤에 게이트 — substrate 가 그래프의 silently-empty 를 방지. 최저 가치 delta(D4)는 defer 하고 regime 생애주기는 불변. gold-plating 없이 server-less 유용성을 실질 개선하는 최소 설계.

## 수용 기준 (Acceptance Criteria)

1. `v34_memory_graph` 마이그레이션이 `memory_claims` + `memory_edges`(`src`/`dst` 인덱스 포함) 생성; `CURRENT_VERSION = 34`; v33 DB 로부터 round-trip 마이그레이션 테스트 통과.
2. `MemoryGraphPort`가 `maekon-core/src/ports/`에 정의되고 `SqliteStorage`가 구현; `clm`/`edg` prefix 가 ADR-022 에 등록.
3. **Phase 1, LLM 없음(NoOpAnalysisProvider 배선):** digest 실행 시 `segment_id`로의 `evidence` 엣지를 가진 `memory_claims` 행 ≥1 생성, 각 claim 이 유효한 `kind`(D5) 태그 보유, `DigestExporter::to_markdown`이 누적 claim 렌더. `llm_api` 미설정 오프라인에서 검증.
4. **Phase 1 durability:** digest 재생성에도 claim 영속(재실행이 기존 active claim 을 삭제/중복하지 않음).
5. **Phase 2, 로컬 Ollama 설정:** relation-extraction 패스가 `supports`/`refines`/`contradicts` 엣지(D1) emit, claim 쌍 모순이 패자를 `status = superseded`로 provenance 엣지와 함께 전이(D2) — `Regime` 행 수정 없이.
6. **Degradation:** LLM 미설정 시 Phase-2 컴포넌트가 `FallbackAnalysisProvider → NoOpAnalysisProvider`로 깨끗이 복귀(panic 없음), Phase-1 그래프는 non-empty 유지.
7. `maekon-analysis`가 no-network-dependency 불변 유지(`reqwest`/`maekon-network` 미추가); 헥사고날 경계(`→ maekon-core`만) 보존. `cargo check/test/clippy/fmt`가 `docs/STATUS.md` 기준 통과.
8. ADR-012/ADR-018 regime merge 동작 불변(`RegimeManager::merge_two` 회귀 테스트).
9. ADR 이 `docs/architecture/README.md`에 등재; 한글 동반본 `ADR-023-local-symbolic-memory-graph.ko.md` 작성.

## PII 완화 조치 (PII Mitigations)

메모리-그래프 경로는 사용자 컨텍스트 텍스트가 LLM 경계를 넘는 세 지점을 도입한다.
각 지점에는 코드베이스 전반에서 사용되는 완화 식별자(`MG-PII-NN`)가 부여된다.

| ID | 경계 | 완화 조치 |
|----|------|-----------|
| **MG-PII-01** | `BeliefRevision` claim 텍스트 → enrichment provider 전송 | 각 claim 텍스트를 provider 용 직렬화 전에 `BeliefRevision` 내부에서 per-claim 마스킹 적용. `crates/maekon-analysis/src/belief_revision.rs` 구현. |
| **MG-PII-02** | `AnalysisClient::new_local_enrichment` — enrichment egress 게이트 | DNS-rebind 강화 루프백 핀: 엔드포인트 호스트를 한 번 리졸브하여 **모든** 리졸브 주소가 루프백임을 단언. 비-루프백 엔드포인트는 생성 시점에 거부(fail-closed). `crates/maekon-network/src/analysis_client/mod.rs` 구현. |
| **MG-PII-03** | `BeliefRevision`이 적용하는 `PiiFilter` 경계 마스커 | 주 분석 경로와 동일한 `PiiFilter` + `VisionPiiSanitizer` 스택. enrichment provider 전송 전 적용. |
| **MG-PII-04** | `build_local_ollama_summary_provider` — segment-summary 루프백 경로 | segment-summary 경로는 `GuardedAnalysisProvider`(active-window 게이트 + `privacy.external_llm.*` 감사 이벤트)를 **경유하지 않는다**. 근거: ① 루프백 = 기기내 egress 경계(MG-PII-02 선례, `new_local_enrichment` fail-closed 게이트 적용); ② `pii_filter_summ` + `VisionPiiSanitizer`는 주 경로와 동일하게 적용; ③ `llm_summary_enabled` + `embedding.enabled` 이중 opt-in(둘 다 기본값 `false`). 잔여 델타 정직 기술: in-process 룰-매처와 달리 **프롬프트 텍스트가 프로세스 재시작 후에도 Ollama 데몬 로그에 잔존할 수 있다**. Ollama가 Maekon과 동일한 OS 사용자로 실행되고 루프백 경계가 기기 외부 egress를 방지하므로 이 델타는 수용한다. `src-tauri/src/agent_runtime/analysis_helpers.rs` 구현. |

## 관련 문서 (Related Docs)

- Andrej Karpathy, *LLM Wiki* — 이 ADR 이 벤치마크하는 원본 패턴(로컬·LLM 유지 markdown 지식베이스): https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f
- `docs/architecture/ADR-011-standalone-analysis-pipeline.md` — server-optional 파이프라인 baseline
- `docs/architecture/ADR-012-adaptive-tiered-memory.md` — regime/segment 생애주기(여기서 불변)
- `docs/architecture/ADR-013-llm-summary-vector-rag.md` — 그래프가 augment 하는 임베딩 + RRF 검색
- `docs/architecture/ADR-018-regime-manager-persistence.md` — `v31` 마이그레이션 선례
- `docs/architecture/ADR-022-client-id-generation-ulid.md` — `generate_id` + prefix 레지스트리(`clm`/`edg`)
- `crates/maekon-analysis/src/claim_promoter.rs` — Phase-1 D3/D5 promoter (digest timeline + LLM highlight → claim/edge)
- `crates/maekon-analysis/src/daily_insight_generator.rs` — LLM `DigestHighlight` source (Phase-1 LLM-on 경로 전용; 오프라인 D3 는 timeline 사용)
- `crates/maekon-analysis/src/fallback_analysis_provider.rs` — NoOp degradation 계약
