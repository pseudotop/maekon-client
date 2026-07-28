[English](./ADR-032-memory-graph-generation-input-contract.md) | [한국어](./ADR-032-memory-graph-generation-input-contract.ko.md)

# ADR-032: Memory-Graph Generation-Input 계약

**상태**: Proposed
**Date**: 2026-07-25
**Scope**: `maekon-analysis` (retrieval, coaching, belief revision), `maekon-suggestion`, `maekon-core` (`ports/memory_graph_port.rs`, `consent.rs`, `models/prompt_assembly.rs`), `maekon-network` (`analysis_client`), `src-tauri` (agent runtime 배선)
**Related**: ADR-023 (Local Symbolic Memory-Graph), ADR-030 (Work Context Envelope) §11, ADR-024 (Conversation Content Guard Port), ADR-026 (ConsentManagerPort), ADR-013 (LLM Summary + Vector RAG), ADR-012 (Adaptive Tiered Memory), ADR-011 (Standalone Analysis Pipeline)
**Issue**: #8087

---

## 배경 (Context)

ADR-023은 로컬 symbolic memory-graph 기반(`memory_claims`, `memory_edges`, `MemoryGraphPort`)과 belief-revision 루프를 도입했다. 그리고 §5에서 명시적 경계를 기록했다 — **claim은 생성(generation)에 입력되지 않는다.** 2026-07 기능 감사는 `coaching_engine`과 `maekon-suggestion`이 `MemoryGraphPort`를 0회 참조하며, ADR이 이미 규정한 retrieval augmentation조차 미배선임을 확인했다. ADR-023 §5는 "claims-as-generation-input"을 *새로운 미비준 설계*로 규정하고 별도 ADR로 연기했다. ADR-030 §11은 독립적으로 memory-graph 생성 정책이 "#8087의 소유로 남는다"고 명시했다. 본 ADR이 그 소유권을 이행한다.

2026-07-25 코드 실측은 연기 상태가 그대로 유지됨을 확인했고, 본 계약이 모순이 아니라 확장해야 할 기준선을 확정했다.

- **LLM 경로는 정확히 하나이며 닫힌 루프다.** `BeliefRevision::run_pass`(`crates/maekon-analysis/src/belief_revision.rs:85-214`)가 `list_claims_by_status(Active)`를 읽어 claim text를 enrichment provider로 보내고, 결과를 다시 edge와 status 전이로만 환원한다. 사용자에게 보이는 생성물로 나가는 것은 없다.
- **나머지 read는 전부 표시 또는 유지보수다.** `handlers/daily_digest.rs:76-85`와 `services/memory_claims_service.rs`는 로컬 대시보드 렌더, `scheduler/loops/system.rs:805-812`는 prune이다. `ContextAssembler`(`assembler.rs`), `prompts.rs`, `few_shot_selector.rs`, `query_expander.rs`, `hybrid_search_service.rs`, `vector_retriever.rs`에는 memory-graph 참조가 **0건**이다.
- **그 하나뿐인 경로마저 기본값 3중 off다**: `belief_revision_enabled = false`(`config/sections/analysis.rs:64`) AND `ConsentPermissions.memory_graph_enrichment = false`(`consent.rs:56-63` — `full_text_extraction`/`activity_pattern_learning`에서 의도적으로 상속하지 않는 Tier-7 전용 권한) AND `llm_api` 미설정 시 `NoOpAnalysisProvider`.
- **그 경로는 loopback 고정이며, 그것이 면제의 근거다.** `AnalysisClient::new_local_enrichment`(`analysis_client/mod.rs:232-247`)는 DNS-rebind 강화 resolve-and-assert로 비-loopback 엔드포인트를 생성 시점에 거부하고, `extract_relations`/`detect_contradictions`가 전송 전 재검사한다. egress가 device-local이기 때문에 ADR-023 MG-PII-04는 `GuardedAnalysisProvider` 우회를 허용하며, **egress ledger 기록도 없다** — `crates/maekon-network/src/analysis_client/`에 `record_egress` 참조 0건인 반면, 실제 off-device 경로(`scheduler/egress_policy.rs`, `guarded_conversation.rs`, `remote_embedding_client.rs`)는 전부 기록한다.
- **현재 투사(projection)되는 필드는 좁다**: `belief_revision.rs:100-104`은 `[(claim_id, pii_masked_text)]`만 직렬화한다. `kind`, `source`, `confidence`, `status`, 타임스탬프, **edge 데이터 전부**는 프로세스 내부에 머문다.
- **프롬프트에 중요한 차원에서 선택이 무한하다**: status는 `Active`로 걸러지지만 개수 상한·토큰 예산·최신성 윈도우·입력측 confidence 하한이 전부 없다. 유일한 하한은 `active.len() < 2 → return`이다.

이 사실들이 본 ADR이 해소해야 할 위험 비대칭을 규정한다. 현행 루프가 안전한 이유는 claim text가 본질적으로 저위험이어서가 아니라, 루프가 **닫혀 있고 로컬이며 기본 off**이기 때문이다. claim text는 화면 활동에서 증류된 사용자 파생 콘텐츠다. 이를 사용자 가시 생성물로 흘리면 현행 설계가 한 번도 넘지 않은 경계를 넘게 되고, 원격 provider로 흘리면 loopback 고정이 벌어들인 모든 면제가 동시에 무효가 된다.

ADR-023 §5는 나아가 흔히 혼동되는 세 설계를 지목한다 — retrieval re-rank, prompt-context injection, coaching gate signal. 프라이버시 표면이 실질적으로 다르므로 단일 승인을 공유해서는 안 된다.

## 결정 (Decision)

**모드 분리 + fail-closed** 생성 입력 계약을 채택한다. 본 ADR에 런타임 변경은 수반되지 않으며, 향후 소비자가 배선 전 충족해야 할 계약을 고정한다.

### 1. 세 모드 — 개별 게이트, 노출 순서 고정

| 모드 | 생성기에 도달하는 것 | 허용 노출 |
|------|--------------------|----------|
| **A — Retrieval augmentation** | edge 위상만(`src_id`, `dst_id`, `edge_type`, `confidence`) — 기존 `hybrid_search` 결과의 re-rank/확장에 사용 | claim text 없음. 랭킹 영향만. |
| **B — Gate signal** | 파생 스칼라/불리언(예: "최근 7일 활성 contradiction ≥ N") | claim text 없음, ID 없음. |
| **C — Prompt-context injection** | 프롬프트 내부의 PII 마스킹된 claim text | 생성기에 전문 노출. |

모드는 **A → B → C** 순으로만 채택한다. 각 모드는 자체 활성화 결정, 자체 consent 평가(§3), 자체 계약 테스트를 요구한다. **한 모드의 승인은 다른 모드를 함의하지 않는다.** 모드 A는 ADR-023이 이미 의도된 read path로 비준한 설계이고, 모드 C는 ADR-023 §5가 미비준으로 지목한 설계로서 아래 최강 요건을 상속한다.

**근거**: 세 모드는 구현 편의가 아니라 *어떤 콘텐츠가 경계를 넘는가*로 갈린다. 이를 "그래프를 생성에 쓴다"는 단일 스위치로 뭉개는 것이 바로 ADR-023 §5가 거부한 ad-hoc 도입이다.

### 2. 유계 투사 (input selection)

생성 목적으로 그래프를 읽는 모든 모드는 소비자가 `MemoryGraphPort`를 직접 호출하지 않고 **단일 공유 projection 헬퍼**를 경유해야 한다. 투사는 **fail-closed**다 — 어떤 경계든 평가 불가하면 무한 집합이 아니라 빈 집합을 낸다.

투사는 다음을 전부 강제해야 한다:

1. **Status**: `Active`만. `Superseded`/`Retracted`는 하류 필터가 아니라 **선택 시점에** 배제한다.
2. **최신성 윈도우**: 유계 `updated_at` 윈도우. ADR-023의 retention prune(`analysis.embedding.retention_days`, 기본 90 — `scheduler/loops/system.rs:799-821`)은 저장소 하한이지 **생성 윈도우가 아니다**. 생성 윈도우는 독립 설정하며 retention 윈도우 이하여야 한다.
3. **Confidence 하한**: 입력측 최소값. belief revision의 *출력*측 게이트인 `supersede_confidence_threshold`(0.9)와 구분되며, 이를 입력 하한으로 재사용해서는 안 된다.
4. **개수 상한(하드)** + 결정적 정렬 — `updated_at DESC`, `claim_id` tie-break(`memory_claims_service.rs:124-128`이 이미 쓰는 정렬). 동일 그래프 상태에서 생성이 재현 가능하도록 정렬은 전순서여야 한다.
5. **필드 allowlist**: `claim_id`, PII 마스킹된 `text`, `kind`. 금지: `source`, raw `confidence`, `evidence_ref`, 그리고 모든 `segment_id`/`frame_id` provenance. provenance 식별자는 내부 상관 키이며 생성기에 도달해서는 안 된다.
6. **Edge 투사**(모드 A 한정): `edge_type`과 `confidence`가 랭킹에 영향을 줄 수 있다. `evidence_ref`는 어떤 모드에서도 투사 불가.

**근거**: 현행 belief-revision 선택에 개수·최신성 경계가 없는 이유는 소비자가 비용이 일 1회 로컬 LLM 호출인 로컬 자기유지 pass이기 때문이다. 생성 소비자는 토큰 예산·지연 예산·노출 표면을 지니므로, 이 경계들이 부수적이 아니라 하중을 받는 요소가 된다.

### 3. 프라이버시 경계

1. **마스킹은 투사 시점 불변식이다.** PII 마스킹(ADR-023 MG-PII-01/MG-PII-03, 주입된 `PiiFilter` seam의 `sanitize_title_with_level`)은 projection 헬퍼 **내부**에서 적용해야 하며, 어떤 소비자도 마스킹되지 않은 claim text를 얻을 수 없어야 한다. 호출부 마스킹은 불가 — 그것이 신규 소비자가 조용히 건너뛰게 되는 형태다.
2. **Consent는 테이블 단위가 아니라 목적 단위다.** `memory_graph_enrichment`는 *자기유지* 루프(graph → LLM → graph)를 인가한다. 사용자 가시 생성물 공급은 다른 목적이며 이 권한을 빌려서는 안 된다. 모드 A/B는 기존 권한의 확장으로 인가할 수 있으나 확장 사실을 `consent.rs`에 문서화하고 consent UI에 노출해야 한다. **모드 C는 자체 권한을 요구**하며 기본값 `false`, `memory_graph_enrichment`가 세운 Tier-7 선례를 따른다.
3. **원격 egress는 MG-PII-04 면제를 무효화한다.** 현행 `GuardedAnalysisProvider` 우회와 egress ledger 부재는 오직 loopback 고정(`host_is_loopback`, `http_client.rs:79-98`)으로 정당화된다. 따라서:
   - 로컬(loopback) 생성 입력은 기존 면제를 그대로 재사용할 수 있다.
   - **비-loopback provider에 도달할 수 있는 모든 생성 입력은 `GuardedAnalysisProvider`(ADR-024)를 경유해야 하며, 전송 전 등록된 `event_type`으로 egress ledger 기록을 남겨야 한다.** ledger 없는 원격 경로는 미비가 아니라 계약 위반이다.
4. **프롬프트 신뢰 경계.** claim text는 사용자 파생이다. 모드 C에서는 `models/prompt_assembly.rs`를 통해 `UntrustedContent`로 감싸야 하며 `TrustedInstruction` 세그먼트에 나타나서는 안 된다. 이는 기존 형태 공백을 닫는다 — belief-revision 경로는 segmented-prompt 래퍼 없이 provider body를 직접 만든다(`analysis_client/requests.rs:6-31`). 닫힌 루프에서는 용인되지만 출력이 사용자에게 도달하는 경로에서는 아니다.
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

1. **Projection 헬퍼 구현** — §2를 구현하는 `maekon-analysis` 단일 seam + 각 경계가 fail-closed임을 단언하는 계약 테스트. 모든 모드의 선행 조건.
2. **모드 A 배선 (retrieval augmentation)** — edge가 `hybrid_search_service` 결과를 re-rank/확장. ADR-023 §4가 이미 스코프했고 §5가 미배선으로 표시한 read path.
3. **원격 그래프 egress용 ledger event type** — §3.3이 비-loopback 모드 전에 요구. 현재 ledger에 `memory_graph`/`belief_revision` event type이 없다.
4. **belief-revision 경로의 segmented-prompt 채택** — 현재 provider body를 직접 만든다. 닫힌 루프에서는 결함이 아니나, 여기서 먼저 `prompt_assembly`를 채택하면 모드 C가 검증된 seam을 재사용할 수 있다.
5. **Provenance 인용 생성** — 사용자 가시 출력에 evidence(`evidence_ref`, `segment_id`)를 인용하고 싶어지면 자체 ADR이 필요하다. §2.5가 의도적으로 봉쇄한다.

## 관련 문서 (Related Docs)

- `docs/architecture/ADR-023-local-symbolic-memory-graph.ko.md` §4-§5 — 기반, 의도된 read path, 본 ADR이 이행하는 연기
- `docs/architecture/ADR-030-work-context-envelope-convergence.ko.md` §11 — memory-graph 생성 정책을 #8087에 인계
- `docs/architecture/ADR-024-conversation-content-guard-port.ko.md` — §3.3이 요구하는 guard + egress-audit 패턴
- `docs/architecture/ADR-026-async-storage-convergence-consent-port.ko.md` — §3.2 게이트가 사는 `ConsentManagerPort`
- `docs/architecture/ADR-013-llm-summary-vector-rag.ko.md` — 모드 A가 augment하는 RRF retrieval
- `crates/maekon-analysis/src/belief_revision.rs` — 유일한 기존 LLM 경로와 그 선택/마스킹 기준선
- `crates/maekon-core/src/models/prompt_assembly.rs` — §3.4가 요구하는 신뢰 경계 래퍼
