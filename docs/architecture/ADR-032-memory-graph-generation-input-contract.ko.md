[English](./ADR-032-memory-graph-generation-input-contract.md) | [한국어](./ADR-032-memory-graph-generation-input-contract.ko.md)

# ADR-032: Memory-Graph Generation-Input 계약

**상태**: Accepted — 3-loop 리뷰(#9463) 후 2026-07-29 개정·승인; 원 Proposed 2026-07-25
**Date**: 2026-07-25 (Proposed) · 2026-07-29 (Accepted)
**Scope**: `maekon-analysis` (retrieval, coaching, belief revision), `maekon-suggestion`, `maekon-core` (`ports/memory_graph_port.rs`, `consent.rs`, `models/prompt_assembly.rs`), `maekon-network` (`analysis_client`), `src-tauri` (agent runtime 배선)
**Related**: ADR-023 (Local Symbolic Memory-Graph), ADR-030 (Work Context Envelope) §11, ADR-024 (Conversation Content Guard Port), ADR-026 (ConsentManagerPort), ADR-013 (LLM Summary + Vector RAG), ADR-012 (Adaptive Tiered Memory), ADR-011 (Standalone Analysis Pipeline)
**Issue**: #8087

---

## 배경 (Context)

ADR-023은 로컬 symbolic memory-graph 기반(`memory_claims`, `memory_edges`, `MemoryGraphPort`)과 belief-revision 루프를 도입했다. 그리고 §5에서 명시적 경계를 기록했다 — **claim은 생성(generation)에 입력되지 않는다.** 2026-07 기능 감사는 `coaching_engine`과 `maekon-suggestion`이 `MemoryGraphPort`를 0회 참조하며, ADR이 이미 규정한 retrieval augmentation조차 미배선임을 확인했다. ADR-023 §5는 "claims-as-generation-input"을 *새로운 미비준 설계*로 규정하고 별도 ADR로 연기했다. ADR-030 §11은 독립적으로 memory-graph 생성 정책이 "#8087의 소유로 남는다"고 명시했다. 본 ADR이 그 소유권을 이행한다.

2026-07-25 코드 실측은 연기 상태가 그대로 유지됨을 확인했고, 본 계약이 모순이 아니라 확장해야 할 기준선을 확정했다.

- **LLM 경로는 정확히 하나이며 닫힌 루프다.** `BeliefRevision::run_pass`(`crates/maekon-analysis/src/belief_revision.rs:85-214`)가 `list_claims_by_status(Active)`를 읽어 claim text를 enrichment provider로 보내고, 결과를 다시 edge와 status 전이로만 환원한다. 사용자에게 보이는 생성물로 나가는 것은 없다.
- **나머지 read는 전부 표시 또는 유지보수다.** `handlers/daily_digest.rs:76-85`와 `services/memory_claims_service.rs`는 로컬 대시보드 렌더, `scheduler/loops/system.rs:805-812`는 prune이다. `ContextAssembler`(`assembler.rs`), `prompts.rs`, `few_shot_selector.rs`, `query_expander.rs`, `vector_retriever.rs`에는 memory-graph 참조가 **0건**이다. (Proposed 초안은 여기에 `hybrid_search_service.rs`도 인용했으나, 그 파일은 이미 PR #5770에서 dead code로 삭제된 상태였다 — 라이브 retrieval 경로는 `maekon-web`의 `semantic_search_service.rs`이며 마찬가지로 memory-graph 참조 0건이다. Known Follow-up 2 참조.)
- **그 하나뿐인 경로마저 기본값 3중 off다**: `belief_revision_enabled = false`(`config/sections/analysis.rs:64`) AND `ConsentPermissions.memory_graph_enrichment = false`(`consent.rs:56-63` — `full_text_extraction`/`activity_pattern_learning`에서 의도적으로 상속하지 않는 Tier-7 전용 권한) AND `llm_api` 미설정 시 `NoOpAnalysisProvider`.
- **그 경로는 loopback 고정이며, 그것이 면제의 근거다.** `AnalysisClient::new_local_enrichment`(`analysis_client/mod.rs:232-247`)는 DNS-rebind 강화 resolve-and-assert로 비-loopback 엔드포인트를 생성 시점에 거부하고, `extract_relations`/`detect_contradictions`가 전송 전 재검사한다. egress가 device-local이기 때문에 ADR-023 MG-PII-04는 `GuardedAnalysisProvider` 우회를 허용하며, **egress ledger 기록도 없다** — `crates/maekon-network/src/analysis_client/`에 `record_egress` 참조 0건인 반면, 실제 off-device 경로(`scheduler/egress_policy.rs`, `guarded_conversation.rs`, `remote_embedding_client.rs`)는 전부 기록한다.
- **현재 투사(projection)되는 필드는 좁다**: `belief_revision.rs:100-104`은 `[(claim_id, pii_masked_text)]`만 직렬화한다. `kind`, `source`, `confidence`, `status`, 타임스탬프, **edge 데이터 전부**는 프로세스 내부에 머문다.
- **프롬프트에 중요한 차원에서 선택이 무한하다**: status는 `Active`로 걸러지지만 개수 상한·토큰 예산·최신성 윈도우·입력측 confidence 하한이 전부 없다. 유일한 하한은 `active.len() < 2 → return`이다.

이 사실들이 본 ADR이 해소해야 할 위험 비대칭을 규정한다. 현행 루프가 안전한 이유는 claim text가 본질적으로 저위험이어서가 아니라, 루프가 **닫혀 있고 로컬이며 기본 off**이기 때문이다. claim text는 화면 활동에서 증류된 사용자 파생 콘텐츠다. 이를 사용자 가시 생성물로 흘리면 현행 설계가 한 번도 넘지 않은 경계를 넘게 되고, 원격 provider로 흘리면 loopback 고정이 벌어들인 모든 면제가 동시에 무효가 된다.

ADR-023 §5는 나아가 흔히 혼동되는 세 설계를 지목한다 — retrieval re-rank, prompt-context injection, coaching gate signal. 프라이버시 표면이 실질적으로 다르므로 단일 승인을 공유해서는 안 된다.

## 결정 (Decision)

**모드 분리 + fail-closed** 생성 입력 계약을 채택한다. 본 ADR에 런타임 변경은 수반되지 않으며, 향후 소비자가 배선 전 충족해야 할 계약을 고정한다.

### 1. 세 모드 — 개별 게이트, 노출 순서 고정

| 모드 | 모드가 읽고 사용하는 것 | 허용 노출 |
|------|------------------------|----------|
| **A — Retrieval augmentation** | edge 위상(`src_id`, `dst_id`, `edge_type`, `confidence`)을 **프로세스 내부에서** 읽어 기존 retrieval 결과 집합의 re-rank/확장에 사용 (라이브 경로: `maekon-web`의 `semantic_search_service`) | **어떤 생성기에도 아무것도 도달하지 않음.** 랭킹 영향만. endpoint 식별자는 랭킹 계산을 떠나지 않는 join 키다(§2.6). |
| **B — Gate signal** | 파생 스칼라/불리언(예: "최근 7일 활성 contradiction ≥ N") | claim text 없음, ID 없음. |
| **C — Prompt-context injection** | 프롬프트 내부의 PII 마스킹된 claim text | 생성기에 전문 노출. |

모드는 **A → B → C** 순으로만 채택한다. 각 모드는 자체 활성화 결정, 자체 consent 평가(§3), 자체 계약 테스트를 요구한다. **한 모드의 승인은 다른 모드를 함의하지 않는다.** 모드 A는 ADR-023이 이미 의도된 read path로 비준한 설계이고, 모드 C는 ADR-023 §5가 미비준으로 지목한 설계로서 아래 최강 요건을 상속한다.

모드 A는 *생성기 인접(generator-adjacent)*이지 생성기 대면(generator-facing)이 아니다 — 이후 단계가 보는 retrieval 결과를 조형할 뿐, 투사된 값 자체가 LLM으로 전송되지는 않는다. claim 행은 edge endpoint(`claim_id`) 해석에만 접근하며, claim의 `text`·`kind`·`source`는 모드 A에서 결코 읽지 않는다.

**근거**: 세 모드는 구현 편의가 아니라 *어떤 콘텐츠가 경계를 넘는가*로 갈린다. 이를 "그래프를 생성에 쓴다"는 단일 스위치로 뭉개는 것이 바로 ADR-023 §5가 거부한 ad-hoc 도입이다.

### 2. 유계 투사 (input selection)

생성 목적으로 그래프를 읽는 모든 모드는 소비자가 `MemoryGraphPort`를 직접 호출하지 않고 **단일 공유 projection 헬퍼**를 경유해야 한다.

**헬퍼의 위치.** 공개 인터페이스는 `maekon-core` port trait(작업명 `MemoryGraphProjectionPort`)이고, 유계 선택 구현은 `maekon-analysis`에 살며, `src-tauri`가 DI로 배선한다(ADR-023 web-render가 `MemoryGraphPort`를 `WebServerRequiredDeps`로 관통시킨 것과 동일한 Port Instance Sharing). 어댑터 소비자 — 라이브 retrieval 경로를 소유하되 `maekon-analysis` 의존이 없는 `maekon-web`, `maekon-suggestion`, coaching — 는 trait에만 의존한다. 교차 어댑터 crate 의존은 계속 금지다. 승인된 각 모드는 **자체 유계 반환 타입을 가진 자체 trait 메서드**를 추가한다(타입 수준 모드 분리 — 모드 enum 없음, 따라서 한 메서드의 승인이 다른 모드의 노출을 넓힐 수 없다). 모드 A 스케치:

```rust
#[async_trait]
pub trait MemoryGraphProjectionPort: Send + Sync {
    /// Mode A: bounded edge-topology projection for in-process ranking.
    async fn project_edges_for_ranking(&self, now_secs: i64) -> Result<EdgeProjection, CoreError>;
}
```

**Fail-closed의 정밀한 범위.** *경계를 평가할 수 없으면* — config 값 누락/무효, consent authority 사용 불가, 윈도우 해석 불가 — 헬퍼는 `Ok` + **빈 투사**를 반환한다. 진짜 저장소 실패(`MemoryGraphPort`의 `Err(CoreError)`)는 `Err` 그대로 전파한다 — 빈 성공으로 가려서는 안 되며, 그러지 않으면 계약 테스트가 "정책상 거부"와 "저장소 고장"을 구분할 수 없다. 두 동작 모두 계약 테스트 대상이다(Known Follow-up 1).

투사는 다음을 전부 강제해야 한다:

1. **Status**: `Active`만. `Superseded`/`Retracted`는 하류 필터가 아니라 **선택 시점에** 배제한다.
2. **최신성 윈도우**: 유계 `updated_at` 윈도우 — `analysis.memory_graph_projection.generation_window_days`, 시작 기본값 **30**. ADR-023의 retention prune(`analysis.embedding.retention_days`, 기본 90 — `scheduler/loops/system.rs:799-821`)은 저장소 하한이지 **생성 윈도우가 아니다**. 생성 윈도우는 독립 설정하며 retention 윈도우 이하여야 한다.
3. **Confidence 하한**: 입력측 최소값 — `min_input_confidence`, 시작 기본값 **0.5**. belief revision의 *출력*측 게이트인 `supersede_confidence_threshold`(0.9)와 구분되며, 이를 입력 하한으로 재사용해서는 안 된다.
4. **개수 상한(하드)** + 결정적 전순서 정렬 — claim 선택은 `max_claims`(시작 기본값 **64**), `updated_at DESC` + `claim_id` tie-break(`memory_claims_service.rs:124-128`이 이미 쓰는 정렬); edge 선택은 `max_edges`(시작 기본값 **256**), `created_at DESC` + `edge_id` tie-break. 동일 그래프 상태에서 생성이 재현 가능하도록 정렬은 전순서여야 한다 — 모든 경계가 config에 고정되고 구현자 임의 선택이 아니므로 재현성이 성립한다.
5. **필드 allowlist**: `claim_id`, PII 마스킹된 `text`, `kind`. 금지: `source`, raw `confidence`, `evidence_ref`, 그리고 모든 `segment_id`/`frame_id` provenance. provenance 식별자는 내부 상관 키이며 생성기에 도달해서는 안 된다. (모드 A는 edge endpoint 해석을 위해 claim 행에서 `claim_id`만 읽는다 — `text`/`kind`/`source`는 모드 A에서 읽지 않는다.)
6. **Edge 투사**(모드 A 한정): 투사 튜플은 (`src_id`, `dst_id`, `edge_type`, `confidence`)다 — endpoint 포함. 랭킹을 가능하게 하는 join 키이기 때문이다(`Evidence` edge의 `dst_id`는 `segment_id`를 참조할 수 있고, 그것이 edge가 `SemanticSearchResult.segment_id` 행에 join되는 방식이다). §2.5의 provenance 금지와 충돌하지 않는다 — §2.5는 **생성기에 도달하거나 생성 출력에 영속되는 것**을 규율하며, edge endpoint는 랭킹 계산 *내부*에서 소비되고 그 밖으로 노출되어서는 안 된다. `evidence_ref`는 어떤 모드에서도 투사 불가.

모든 경계는 단일 명명 config 섹션에 산다: `analysis.memory_graph_projection`(`MemoryGraphProjectionConfig`, `AnalysisConfig` 안에서 `embedding`의 형제). 위 시작 기본값은 계약적 시작값이다 — config로 조정 가능하되, 필드는 반드시 존재·강제되고 fail-closed 계약 테스트로 커버되어야 한다. 세 모드 소비자가 세 개의 config 경로를 발명하면 §2의 단일 헬퍼 보장이 무효가 된다.

**근거**: 현행 belief-revision 선택에 개수·최신성 경계가 없는 이유는 소비자가 비용이 일 1회 로컬 LLM 호출인 로컬 자기유지 pass이기 때문이다. 생성 소비자는 토큰 예산·지연 예산·노출 표면을 지니므로, 이 경계들이 부수적이 아니라 하중을 받는 요소가 된다.

### 3. 프라이버시 경계

1. **마스킹은 투사 시점 불변식이다.** PII 마스킹(ADR-023 MG-PII-01/MG-PII-03)은 텍스트를 투사하는 모든 모드(현재: 모드 C)에 대해 projection 헬퍼 **내부**에서, 주입된 `maekon_core::ports::pii_sanitizer::PiiSanitizer` seam으로 적용해야 한다 — 워크스페이스의 교차-crate 마스킹 포트이며 `semantic_search_service`가 이미 소비한다. (Proposed 초안이 지목한 belief revision의 사설 `PiiFilter` closure alias는 analysis 내부 편의물이지 본 계약의 seam이 아니다.) 어떤 소비자도 마스킹되지 않은 claim text를 얻을 수 없다. 모드 A/B는 텍스트를 투사하지 않으므로 마스킹 조항이 공허하게 충족된다 — 모드 A에 sanitizer 의존이 필요하다고 오독하지 않도록 명시해 둔다. 호출부 마스킹은 불가 — 그것이 신규 소비자가 조용히 건너뛰게 되는 형태다.
2. **Consent는 목적 단위·모드 단위·전용이다.** `memory_graph_enrichment`(Tier 7)는 *자기유지* 루프(graph → LLM → graph)만을 인가한다. 모든 생성 입력 모드는 **자체 전용 `ConsentPermissions` 불리언**을 요구한다 — `#[serde(default)]`(fail-closed `false`), Tier 4–9가 세운 1-능력-1-권한 관례(각 필드의 doc comment가 무엇에서 빌리지 않는지 명시) 준수. 확정 이름: 모드 A → `memory_graph_retrieval_ranking`(Tier 10), 모드 B → `memory_graph_gate_signal`(Tier 11), 모드 C → `memory_graph_prompt_injection`(Tier 12). 각 필드는 해당 모드를 출하하는 PR에서 본 ADR을 인용하는 doc comment와 함께 추가되며, 어떤 모드도 형제 권한을 빌리거나 "확장"할 수 없다. (Proposed 초안은 모드 A/B의 기존 권한 확장을 허용했으나, 2026-07-29 리뷰가 Alternative C가 기각한 것과 동일한 목적 확장이라며 그 조항을 삭제했다.)
3. **원격 egress는 MG-PII-04 면제를 무효화한다.** 현행 `GuardedAnalysisProvider` 우회와 egress ledger 부재는 오직 loopback 고정(`host_is_loopback`, `http_client.rs:79-98`)으로 정당화된다. 따라서:
   - 로컬(loopback) 생성 입력은 기존 면제를 그대로 재사용할 수 있다.
   - **비-loopback provider에 도달할 수 있는 모든 생성 입력은 `GuardedAnalysisProvider`(ADR-024)를 경유해야 하며, 전송 전 등록된 `event_type`으로 egress ledger 기록을 남겨야 한다.** ledger 없는 원격 경로는 미비가 아니라 계약 위반이다.
4. **프롬프트 신뢰 경계.** claim text는 사용자 파생이다. 모드 C에서는 `models/prompt_assembly.rs`를 통해 `UntrustedContent`로 감싸야 하며 `TrustedInstruction` 세그먼트에 나타나서는 안 된다. 투사된 각 claim은 하나의 blob으로 이어붙이지 않고 **각자의** `UntrustedContent`(label = 해당 `claim_id`)로 감싼다 — provenance가 claim 단위로 추적 가능해야 §4.2의 재평가 의무가 성립한다. 이는 기존 형태 공백을 닫는다 — belief-revision 경로는 segmented-prompt 래퍼 없이 provider body를 직접 만든다(`analysis_client/requests.rs:6-31`). 닫힌 루프에서는 용인되지만 출력이 사용자에게 도달하는 경로에서는 아니다.
5. **Device-local 불변식 유지.** `memory_claims`/`memory_edges`는 크로스디바이스 sync에서 계속 제외한다(`sync_extractor.rs:66-68`). 어떤 생성 입력 모드도 그래프 행의 sync·업로드 경로를 도입할 수 없다.

**근거**: 각 조항은 자신이 보호하는 면제를 명시한다. loopback 고정은 ADR-023의 별개 양보 두 건을 지탱하는 하중 사실이므로, 그 사실이 성립하지 않게 될 때 무엇이 일어나는지를 계약이 분명히 말한다.

### 4. Staleness와 무효화

1. **투사된 claim의 pass 간 캐싱 금지.** 투사는 그것을 소비한 단일 생성에 한해 유효하다. retraction과 supersession은 명시적 캐시 flush 없이 다음 생성에 즉시 반영되어야 한다.
2. **Retraction은 사용자 가시이며 효력이 즉시다.** `POST /api/memory/claims/{id}/retract`(`handlers/memory_claims.rs:58-72`)는 삭제가 아니라 status 전이로 provenance를 보존한다. §2.1이 비-`Active`를 선택 시점에 배제하므로 retraction은 이후 모든 생성에서 해당 claim을 제거한다. 이미 발행된 생성물이 소급 무효화되지는 않으나, 소비자가 claim을 인용한 생성 결과를 영속화한다면 재평가가 가능하도록 `claim_id`를 기록해야 한다.
3. **Supersede되지 않은 contradiction은 조용히 자격을 얻지 않는다.** inbound `Contradicts` edge를 가졌으나 belief-revision pass가 아직 돌지 않았거나 `supersede_confidence_threshold` 미만으로 끝난 claim은 여전히 `Active`다. 모드 C는 미해소 inbound `Contradicts`를 가진 claim을 배제해야 한다. 모드 A/B는 텍스트를 노출하지 않으므로 포함할 수 있다.
4. **Retention은 하한이지 정책이 아니다.** 90일 prune은 저장소를 제한한다. 90일 생성 윈도우를 인가하지 않는다(§2.2).

## 결과 (Consequences)

### 긍정

- 향후 소비자가 열린 질문 대신 실행 가능한 체크리스트를 갖는다. "그래프를 제안에 배선한다"가 폭발 반경이 무한한 한 줄 변경이기를 멈춘다.
- ADR-023이 부여한 loopback 면제가 명시적으로 조건부가 되어, 그래프를 원격 provider로 확장할 때 면제를 조용히 상속할 수 없다.
- 모드 분리 덕에 가장 저렴하고 이미 비준된 성과(모드 A retrieval augmentation)가 모드 C의 consent·egress 요건을 끌고 가지 않고 출하될 수 있다.

### 부정

- 세 모드는 세 번의 활성화 결정과 세 개의 테스트 표면을 뜻한다. 실제로 모드 C가 필요한 소비자는 부분 비용이 아니라 전체 비용을 지불한다.
- projection 헬퍼는 아직 존재하지 않는 간접층이므로 첫 소비자가 구현 비용을 진다.
- 모든 모드에서 `evidence_ref`를 배제하므로 provenance 인용 생성("이 제안은 오후 3시 세션에 근거합니다")은 후속 ADR 없이는 봉쇄된다.

### 중립

- 채택 시 런타임 동작 변화가 없다. 소비자가 별도 승인될 때까지 그래프는 표시 + belief revision 전용으로 남는다.
- 계약이 아직 존재하지 않는 소비자를 구속하므로, 실제 첫 시험은 본 ADR이 아니라 첫 소비자다.

## 검토한 대안 (Alternatives Considered)

**A. 신규 ADR 대신 ADR-023 개정.** 기각. ADR-023은 `Accepted — fully implemented`이며, 미구현 선행 계약을 접붙이면 그 상태가 모호해진다. 또한 이 계약은 ADR-024(guard/egress)·ADR-026(consent)·ADR-013(retrieval)을 함께 구속하는 cross-cutting 사안이지 memory-graph 기반 결정이 아니다. ADR-030 §11이 이미 별도 소유 정책으로 취급한다.

**B. 세 설계를 아우르는 단일 "생성 입력" 스위치.** 기각. ADR-023 §5가 거부한 ad-hoc 도입 그 자체이며, 모드 A의 저위험 승인이 모드 C의 텍스트 노출을 태우게 된다.

**C. 생성 입력에 `memory_graph_enrichment` 재사용.** 기각. 이 권한은 닫힌 자기유지 루프로 스코프됐다. read의 기술적 유사성과 무관하게 consent 권한의 목적 확장은 프라이버시 후퇴다.

**D. MG-PII-04의 `GuardedAnalysisProvider` 우회를 모든 생성 경로로 확대.** 기각. 우회는 loopback 경계로 정당화되지 삽입된 파이프라인의 정체성으로 정당화되지 않는다. 감사 흔적 없는 off-device egress는 그래프를 ledger 기록이 없는 유일한 LLM 인접 표면으로 남긴다.

**E. 본 ADR에서 모드 A를 지금 배선.** 범위상 기각. ADR-023이 이미 retrieval augmentation을 비준했으므로 배선은 본 계약이 규율하되 포함할 필요는 없는 구현 과제다.

## 알려진 후속 작업 (Known Follow-ups)

1. **Projection 헬퍼 구현** — `maekon-core`의 `MemoryGraphProjectionPort` trait + `maekon-analysis`의 유계 선택 구현 + `src-tauri` DI 배선; 소비자는 trait에만 의존한다(§2). 같은 PR이 §2 시작 기본값을 담은 `analysis.memory_graph_projection`(`MemoryGraphProjectionConfig`)과, 두 fail-closed 의미론(평가 불가 경계 → `Ok(빈 투사)`, 저장소 오류 → `Err`)을 단언하는 계약 테스트를 반드시 추가한다. 모든 모드의 선행 조건.
2. **모드 A 배선 (retrieval augmentation)** — edge가 **라이브** retrieval 경로를 re-rank/확장한다: `crates/maekon-web/src/services/semantic_search_service.rs`(`vector_search` / `adaptive_vector_search` / `fuse_keyword_first`), 결과는 `segment_id` 키. (`HybridSearchService` — ADR-023 §4가 스코프했고 Proposed 초안이 인용한 경로 — 는 PR #5770, 커밋 `54ce99de46`에서 dead code로 삭제됐다; ADR-013의 RRF fusion은 이제 `fuse_keyword_first`에 산다.) join은 `dst_id`가 `segment_id`를 참조하는 `EdgeType::Evidence` edge다. 모드 A PR은 join 커버리지 — 아직 해석 가능한 `segment_id`로의 `Evidence` edge를 가진 `Active` claim 수 — 를 먼저 실측해야 한다. 커버리지가 미미하면 모드 A는 랭킹 변화를 내지 않으며, 그것은 감출 결함이 아니라 정직하게 보고할 수용 가능한 fail-closed 결과다. 모드 A는 `memory_graph_retrieval_ranking`(Tier 10, §3.2)도 함께 추가한다.
3. **원격 그래프 egress용 ledger event type** — §3.3이 비-loopback 모드 전에 요구. 현재 ledger에 `memory_graph`/`belief_revision` event type이 없다.
4. **belief-revision 경로의 segmented-prompt 채택** — 현재 provider body를 직접 만든다. 닫힌 루프에서는 결함이 아니나, 여기서 먼저 `prompt_assembly`를 채택하면 모드 C가 검증된 seam을 재사용할 수 있다.
5. **Provenance 인용 생성** — 사용자 가시 출력에 evidence(`evidence_ref`, `segment_id`)를 인용하고 싶어지면 자체 ADR이 필요하다. §2.5가 의도적으로 봉쇄한다.
6. **Stale retrieval 경로 참조** — `CLAUDE.md` crate summary와 `docs/crates/maekon-analysis.md`는 본 개정과 함께 수정된다; ADR-013/ADR-023 본문은 여전히 `hybrid_search_service`를 언급한다. 역사적 ADR은 그대로 두되(PR #5770 이전 문서), 신규 문서는 `semantic_search_service`를 인용해야 한다.

## 개정 이력 (Amendment History)

- **2026-07-29 (#9463, 3-loop 리뷰: devils-advocate + 구현자 렌즈; BLOCKING/IMPORTANT 발견 전량 반영):**
  1. §3.2 재작성 — 삭제된 "기존 권한 확장" 조항을 모드별 전용 consent 권한(Tier 10–12, 이름 확정)으로 대체. Alternative C 및 Tier 4–9의 1-능력-1-권한 관례와 정합 복원.
  2. §2 경계 고정 — 명명 config 섹션 `analysis.memory_graph_projection`과 계약적 시작 기본값(윈도우 30d ≤ retention, 하한 0.5, 상한 64/256) + edge 전순서 정렬.
  3. Projection 헬퍼 위치 확정 — `maekon-core` port trait + `maekon-analysis` 구현 + `src-tauri` DI; 소비자는 trait에만 의존(Proposed의 "단일 `maekon-analysis` seam" 표현은 `maekon-analysis` 의존이 없는 `maekon-web`에서 구조적으로 호출 불가였다).
  4. 모드 A 통합 지점 정정 — `HybridSearchService`는 이미 삭제됨(PR #5770); 라이브 seam은 `semantic_search_service`, join 키는 `Evidence.dst_id → segment_id`, 모드 A PR에 join 커버리지 실측 의무 부과.
  5. §1 vs §2.5/§2.6 endpoint 노출 모순 해소 — endpoint는 프로세스 내부 join 키이며 생성기에 결코 노출되지 않는다; 모드 A는 claim 행에서 `claim_id`만 접근.
  6. Fail-closed 의미론 분리 — 평가 불가 경계 → `Ok(빈 투사)`; 저장소 오류 → `Err`; 타입 수준 모드 분리를 갖춘 port 시그니처 스케치 추가.
  7. §3.1 마스킹 seam을 `PiiSanitizer` 코어 포트로 정정; 모드 A/B 명시 carve-out(텍스트 미투사).
  8. §3.4 모드 C의 claim별 `UntrustedContent` 래핑 확정.

## 관련 문서 (Related Docs)

- `docs/architecture/ADR-023-local-symbolic-memory-graph.ko.md` §4-§5 — 기반, 의도된 read path, 본 ADR이 이행하는 연기
- `docs/architecture/ADR-030-work-context-envelope-convergence.ko.md` §11 — memory-graph 생성 정책을 #8087에 인계
- `docs/architecture/ADR-024-conversation-content-guard-port.ko.md` — §3.3이 요구하는 guard + egress-audit 패턴
- `docs/architecture/ADR-026-async-storage-convergence-consent-port.ko.md` — §3.2 게이트가 사는 `ConsentManagerPort`
- `docs/architecture/ADR-013-llm-summary-vector-rag.ko.md` — 모드 A가 augment하는 RRF retrieval(그 fusion은 이제 `semantic_search_service::fuse_keyword_first`에 산다; Known Follow-up 2 참조)
- `crates/maekon-web/src/services/semantic_search_service.rs` — 모드 A의 라이브 통합 지점(`segment_id` 키 결과)
- `crates/maekon-analysis/src/belief_revision.rs` — 유일한 기존 LLM 경로와 그 선택/마스킹 기준선
- `crates/maekon-core/src/ports/pii_sanitizer.rs` — §3.1이 요구하는 교차-crate 마스킹 seam
- `crates/maekon-core/src/models/prompt_assembly.rs` — §3.4가 요구하는 신뢰 경계 래퍼
