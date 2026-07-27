[English](./ADR-028-durable-task-lifecycle-boundary.md) | [한국어](./ADR-028-durable-task-lifecycle-boundary.ko.md)

# ADR-028: 영속 Task Lifecycle 경계

**상태**: Accepted (2026-07-21 3-loop 리뷰 후 개정 — [개정 2026-07-21](#개정-2026-07-21-리뷰-해소) 참조)
**일자**: 2026-07-19
**범위**: `maekon-core` task 모델·포트, `maekon-storage` SQLite 어댑터, `src-tauri` task 유스케이스·IPC, `maekon-web` task 뷰
**관련**: ADR-001 (crate 의존 방향), ADR-006 (IPC 계약), ADR-022 (prefix+ULID ID), ADR-026 (비동기 storage·consent 포트), ADR-027 (파생 suggestion action binding)
**이슈**: #8576 (`MK-CONTEXT-01.T01`)

---

## 배경

Maekon은 consent-gated 현재 로컬 scene에서 파생한 제안을 포함해 일시적인
suggestion을 생성할 수 있다. Suggestion은 영속 task가 아니다. 기존
`SuggestionType`은 SQLite, Serde, protobuf, UI 전체에서 동결돼 있고, 그
feedback lifecycle은 사용자가 확정한 업무, blocker, 재시작 복구, source
loss를 표현하지 않는다.

분리 계약 없이 task persistence를 추가하면 다음 세 가지 모호성이 생긴다.

1. 생성된 텍스트가 명시적 사람의 행위 없이 영속 claim이 될 수 있다.
2. 확인 중 retry 또는 crash가 중복 task를 만들 수 있다.
3. OCR/extension source 자료가 task record를 통해 consent·retention 경계보다
   오래 남을 수 있다.

이 ADR은 task 구현 전 영속 경계를 정의한다. `SuggestionType` 변경, storage
schema 구현, 외부 task 동기화는 포함하지 않는다. Proposed ADR은 리뷰
대상이지만 아직 효력이 없다. ADR이 Accepted 되기 전에 #8577은 이 계약을
구현하면 안 된다.

## 결정

### 1. Candidate와 Todo는 서로 다른 claim이다

`TaskCandidate`는 검토 가능한 제안이고, `TodoItem`은 사용자가 확정한
record다. 생성은 candidate를 만들 수 있지만, 명시적인 사람의 확인만
Todo를 만들 수 있다. confidence threshold, timer, retry, sync message,
extension, automation policy는 confirmation을 대신할 수 없다.

모델은 `maekon-core`에 둔다. ADR-022에 추가되는 prefix는 다음과 같다.

| Entity | Prefix | 의미 |
|---|---|---|
| `TaskCandidate` | `tcand` | 검토 가능하고 비-authoritative인 제안 |
| `TodoItem` | `todo` | 사람이 확정한 영속 task |
| transition receipt | `tmut` | idempotent mutation receipt |

구현은 `generate_id`를 사용해야 하며 adapter나 WebView가 ID를 발급하면 안
된다. 이 ADR의 Accepted 전환은 위 prefix를 등록한다. ADR-022의 ID 형식과
validation 규칙은 그대로 유지한다.

영속 shape는 의도적으로 작게 유지한다.

| Model | Identity/state 이외 사용자·data field |
|---|---|
| `TaskCandidate` | sanitized title/body, 선택적 proposed due time·opaque owner reference, expiry, `TaskSourceRef`, revision/timestamp |
| `TodoItem` | sanitized title/body, 선택적 due time·opaque owner reference, unique origin candidate, 선택적 `supersedes_todo_id`, revision/timestamp |
| blocker | 기존 Todo 사이의 directed `blocked_todo_id -> blocker_todo_id` edge |

Owner reference는 local opaque identifier이며 email, display name, token,
account subject가 아니다. Proposed due/owner 값은 사람이 확인하기 전까지
proposal이다. Blocker는 confirm 이후 explicit mutation으로만 추가하며 생성된
prose에서 추론하지 않는다.

### 2. 상태 머신은 명시적이며 fail-closed다

Candidate transition:

```text
proposed ──confirm──> confirmed
    ├──────dismiss──> dismissed
    └──────expiry───> expired
```

`confirmed`, `dismissed`, `expired`는 terminal이다. confirmed candidate에는
정확히 하나의 origin Todo가 있고, dismissed/expired candidate에는 없다.

Todo transition:

```text
confirmed ──start────> in_progress ──wait────> waiting
    │          │              │          └──resume──> in_progress
    │          │              ├──────────────done───> done
    │          │              └────────────cancel───> cancelled
    │          ├─────────────────────────────done───> done
    │          └──────────────────────────cancel────> cancelled
    ├───────────────────────────────────────done────> done
    ├──────────────────────────────────────wait─────> waiting
    └──────────────────────────────────────cancel───> cancelled
```

다음 transition matrix가 authoritative하다.

| From | 허용 destination |
|---|---|
| `confirmed` | `in_progress`, `waiting`, `done`, `cancelled` |
| `in_progress` | `waiting`, `done`, `cancelled` |
| `waiting` | `in_progress`, `done`, `cancelled` |
| `done` | 없음 |
| `cancelled` | 없음 |

Self-transition, 역방향 transition, reopen은 금지한다. 미래 reopen flow는
명시적인 `supersedes_todo_id`를 가진 새 `TodoItem`을 만들며 terminal
history를 변경하지 않는다. 알 수 없는 state 값은 export를 위해 읽을 수
있지만, 새 client가 이해하기 전까지 mutation은 금지한다.

### 3. Confirmation과 모든 transition은 transaction·idempotent다

모든 command는 opaque `idempotency_key`와 `expected_revision`을 가진다.
정합성은 process-local single-flight가 아니라 SQLite transaction과
compare-and-swap에서 나온다.

Confirmation은 하나의 transaction이다.

1. 같은 receipt가 있으면 재생한다.
2. candidate state와 revision을 비교한다.
3. revision을 증가시키며 `proposed -> confirmed`로 갱신한다.
4. `origin_candidate_id`가 unique인 `TodoItem` 하나를 삽입한다.
5. request/result metadata만 가진 receipt를 삽입한다.

Crash 시 다섯 효과가 모두 commit되거나 모두 사라진다. 같은 key와 같은
request를 재생하면 최초 결과를 반환한다. 같은 key를 다른 request content로
재사용하면 `idempotency_mismatch`를 반환한다. 승리한 transition 뒤 다른
key가 race하면 `revision_conflict` 또는 `already_transitioned`를 반환하고
두 번째 Todo는 만들지 않는다.

Candidate dismiss/expiry와 Todo transition에도 같은 규칙을 적용한다.
Receipt는 `(entity_kind, entity_id, idempotency_key)`로 unique하며 canonical
request hash, from/to state, result revision, 선택적 result entity ID를 가진다.
Title, body, OCR, source subject identifier는 포함하지 않는다.

### 4. `TaskSourceRef`는 immutable provenance이며 source content가 아니다

모든 candidate는 생성 시 정확히 하나의 source reference를 가진다. Identity
field는 다른 source를 가리키도록 바꿀 수 없다. Lifecycle과 outcome field만
단조롭게 진행할 수 있다.

| Field | 계약 |
|---|---|
| `source_kind` | `local_current_scene`, `interruption`, `extension_context`, 또는 forward-compatible unknown 값 |
| `extension_id` | 선택적 stable extension type ID; executable path 금지 |
| `install_id` | 선택적 opaque installation ID; credential 금지 |
| `account_subject_ref` | 선택적 opaque account subject; email, display name, token, secret 금지 |
| `upstream_object_id` | 선택적 source-owned object ID |
| `upstream_revision` / `upstream_etag` | 선택적 opaque source version; authority를 부여하지 않음 |
| `occurred_at` | 알 수 있다면 source event 발생 시각 |
| `observed_at` | Maekon 관측 시각 |
| `dedupe_namespace` | source subject 범위 stable non-secret namespace |
| `content_hash` | canonical sanitized candidate content의 `sha256:<hex>`; raw OCR bytes 금지 |
| `lifecycle` | `active`, `deleted`, `access_revoked`, `retention_expired`; `active`에서 멀어지는 단조 진행 |
| `source_outcome` | 선택적 `pending`, `resumed`, `abandoned`, `expired`; `interruption`에만 유효 |

알 수 없는 `source_kind`와 lifecycle 값은 storage와 export를 왕복해야 한다.
Acquisition과 mutation에는 fail-closed다. Source reference는 ACL, consent
token, access token, screenshot, accessibility tree, raw OCR, app/window title,
extension payload를 운반하지 않는다.

#8583이 소유할 미래 `WorkContextEnvelope`가 이 reference를 채울 수 있지만
task state를 소유하지 않는다. `TaskSourceRef`는 그 envelope에서 만든 immutable,
최소화 projection이다. Task는 #8583 구현에 의존하지 않고 envelope 자체를
persist하지 않는다.

### 5. Source retry는 dedupe하지만 비슷한 task를 자동 병합하지 않는다

Candidate 생성은 다음 값을 계산한다.

```text
SHA256(
  "task-candidate/v1\0" || dedupe_namespace || source_kind ||
  extension_id? || install_id? || account_subject_ref? ||
  upstream_object_id? || upstream_revision? || content_hash
)
```

Canonical encoding은 length-prefixed UTF-8이며 optional 값 부재를 명시하는
marker를 사용한다. 결과 `dedupe_key`는 local store에서 전역 unique다. 같은
source revision의 재전달은 candidate가 dismissed/expired여도 기존 candidate를
반환한다. Retry가 이를 되살리면 안 된다. 새 upstream revision 또는 변경된
sanitized content는 새 key를 만든다.

이 key는 at-least-once ingestion guard이며 semantic task matching이 아니다.
비슷한 title, embedding, LLM 판단은 candidate/Todo를 자동 병합하지 않는다.

### 6. Privacy와 retention은 더 엄격한 source 경계를 따른다

Task persistence에는 sanitized candidate title/body, 사용자 확정 Todo field,
최소화 source provenance, transition metadata만 저장한다. Raw OCR, screenshot,
accessibility text, full extension payload, secret은 task table이나 audit event에
기록하지 않는다.

v1 retention 규칙은 다음으로 고정한다.

| Data | Retention |
|---|---|
| proposed candidate content | `expires_at`까지. 생성 후 7일 또는 더 이른 source expiry보다 늦을 수 없음 |
| dismissed/expired candidate content | terminal transition transaction에서 제거; metadata tombstone 30일 유지 |
| confirmed candidate content | 새 Todo에 복사 후 confirmation transaction에서 제거; Todo가 존재하는 동안 provenance tombstone 유지 |
| active Todo (`confirmed`, `in_progress`, `waiting`) | 사용자 transition 또는 explicit delete까지 |
| terminal Todo (`done`, `cancelled`) | 기본 90일, 사용자가 단축 가능; 새 ADR 없이는 최대 365일 |
| transition receipts | entity 존재 중 유지, 그 뒤 retry convergence용 최대 30일 |

Consent revoke 또는 source access loss 시:

- 새 acquisition은 task 생성 전에 중단한다.
- 해당 consent/source의 proposed candidate는 typed reason으로 expire하고 같은
  transaction에서 content를 제거한다.
- 사용자가 확정한 Todo는 조용히 삭제하지 않고 source lifecycle을
  `access_revoked` 또는 `retention_expired`로 진행한다.
- install/account/upstream identifier, revision, etag, dedupe namespace를
  제거한다. Source kind, lifecycle, occurred/observed timestamp, non-content
  transition history는 provenance 설명에 충분히 남긴다.
- explicit full erasure는 candidate, source ref, Todo, blocker, receipt를
  삭제한다. Retain되는 audit/egress record에는 category와 outcome만 남고,
  entity ID, title, body, hash, source identifier는 남기지 않는다.

개인정보 export는 기존 export·masking 경로를 통해 모든 task table을 포함해야
한다. Full erasure는 canonical `ALL_TABLES` delete family에 task table을
child-first 순서로 추가해야 한다. 어느 경로도 존재하지 않는 raw source data를
복원하거나 존재한다고 암시하면 안 된다.

### 7. Restart reconciliation은 사용자 의도를 만들어내지 않는다

Startup은 migration 이후에만 store를 열고 하나의 idempotent task reconciliation
transaction을 실행한다. `effective_now = max(current_utc,
persisted_last_reconciled_at)`를 사용해 wall-clock rollback이 candidate를
un-expire하지 못하게 한다. Persisted floor는 성공한 reconciliation 뒤에만
진행한다.

| Restart 후 관측 상태 | 필수 결과 |
|---|---|
| `expires_at <= effective_now`인 proposed candidate | `expired` transition, content 제거, deterministic reconciliation receipt 기록 |
| TTL 안의 proposed candidate | 상태 변경 없음 |
| confirmed candidate + origin Todo 하나 | 상태 변경 없음; receipt replay 유효 |
| Todo 없는 confirmed candidate | integrity error; quarantine/read-only failure, Todo 자동 생성 금지 |
| candidate당 Todo가 두 개 이상 | schema integrity failure; fail-closed |
| receipt commit됨 | 저장된 결과 replay |
| transaction 미-commit으로 receipt 없음 | original key로 caller retry 가능 |
| unknown state/source lifecycle | export용 보존; mutation 거부 |

Interruption source의 의도는 다음처럼 명시한다.

- `pending`: resume/abandon 결정이 없고 candidate가 아직 proposed다.
- `resumed`: source에 `resumed_at`이 기록됐다. 미확정 restore candidate는
  `source_resumed` 사유로 dismiss한다.
- `abandoned`: 사용자가 명시적으로 restore하지 않기로 했거나 다른 task를
  명시적으로 선택했다. 시간 경과만으로 abandoned라고 판단하지 않는다.
- `expired`: 아무 결정 없이 candidate TTL이 지났다.

확정된 Todo는 이후 interruption outcome에 따라 변경하지 않는다.
Reconciliation은 최소화 provenance를 갱신할 수 있지만 confirmation,
completion, cancellation을 추론하면 안 된다.

### 8. Hexagonal port와 IPC는 authority를 application 내부에 둔다

순수 state transition 함수와 모델은 `maekon-core`에 둔다. `maekon-core`는
narrow object-safe async `TaskCommandPort`와 `TaskQueryPort` trait를 노출한다.
`maekon-storage`는 구현만 하고 transition을 결정하지 않는다. `src-tauri`
application use case가 source ref를 만들고 live consent를 검증하고 port를
호출하며 commit 뒤에만 event를 발행한다. `maekon-web`은 sanitized DTO만 받고
SQLite를 직접 호출하거나 source provenance를 제공하지 않는다.

최소 IPC surface는 다음과 같다.

- candidate와 Todo list/get
- `candidate_id`, `expected_revision`, `idempotency_key`로 candidate confirm
- 같은 concurrency field와 bounded reason으로 candidate dismiss
- `todo_id`, target state, expected revision, key로 Todo transition
- explicit Todo delete
- 기존 privacy command를 통한 export/erase

IPC는 `confirmed`, `dismissed`, `expired`, `revision_conflict`,
`already_transitioned`, `idempotency_mismatch`, `consent_required`,
`source_unavailable` 같은 typed result를 반환한다. Client는 mutation command에
source reference, `origin_candidate_id`, receipt result, raw context를 제공할 수
없다. Blocker edit도 동일한 revision/idempotency 계약을 따르며 self-link와
cycle을 거부한다.

### 9. SQLite migration은 additive이며 rollback은 restore 기반이다

구현은 다섯 table을 추가한다.

| Table | 필수 불변식 |
|---|---|
| `task_candidates` | state CHECK, revision, expiry, sanitized nullable content, unique dedupe key |
| `task_source_refs` | candidate당 정확히 한 row, forward-compatible source 값, raw payload 없음 |
| `todo_items` | state CHECK, revision, `origin_candidate_id UNIQUE NOT NULL` |
| `todo_blockers` | unique directed edge, self-edge 금지; application이 cycle 거부 |
| `task_transition_receipts` | unique entity/key tuple, canonical request hash, metadata-only result |

Foreign key는 child-first cascade를 쓰지만 explicit erasure list는 여전히
필수다. Index는 candidate state/expiry, Todo state/update time, source
object/revision lookup, receipt replay를 커버한다. v1에서 free-text field를 FTS나
sync table에 넣지 않는다.

Baseline commit `11050b0`에서 `CURRENT_VERSION`은 48이므로 예상 slot은 v49다.
구현 이슈는 현재 `main`에서 다음 미사용 version을 다시 확인해야 하며, 이 ADR은
낡을 수 있는 번호를 예약하지 않는다. Migration은 additive이고 기존
pre-migration backup 뒤 per-version savepoint 안에서 실행하며 fresh DB, v48
upgrade, injected failure, future schema test를 포함한다. Suggestion이나
interruption backfill은 없다.

구버전 binary가 더 새로운 schema를 만나면 계속 fail-closed한다. Rollback은
pre-migration backup 복원 또는 compatible binary 배포를 의미한다.
`user_version`을 낮추거나 task table을 제자리에서 drop하거나 Todo를 Suggestion으로
재해석하지 않는다. Command registration은 data 삭제 없이 feature-disable할 수
있다.

## 동결 불변식

다음 항목 변경은 새 ADR 또는 본 ADR의 명시적 update가 필요하다.

1. 명시적 사람의 확인 없는 영속 `TodoItem` 금지
2. candidate 하나가 만드는 origin Todo는 최대 하나
3. `SuggestionType`은 동결된 10-variant 계약 유지
4. raw scene/extension context는 task persistence/task audit에 진입 금지
5. source retry는 dismissed/expired candidate를 되살릴 수 없음
6. unknown state/source 값은 mutation에 fail-closed, export 가능
7. restart reconciliation은 confirmation/completion을 만들어내지 않음
8. 별도 sync ADR Accepted 전까지 task persistence는 local-only

## 결과

### 긍정

- Human confirmation이 UI 관례가 아니라 구조적 경계가 된다.
- Transaction receipt가 retry와 crash recovery를 deterministic하게 만든다.
- 최소화 source reference가 raw 자료를 보존하지 않고 미래 extension을
  지원한다.
- Privacy export, revoke, retention, erasure 의무를 구현 전 table 수준으로
  검증할 수 있다.

### 부정

- 기존 suggestion row 확장보다 다섯 table과 두 narrow port 구현 비용이 크다.
- Terminal record에는 scheduled retention reconciliation이 필요하다.
- Confirmed task가 source access보다 오래 살 수 있으므로 UI는 원본을 열 수
  있다고 약속하지 않고 최소화 provenance를 설명해야 한다.

### 중립

- Suggestion feedback과 task transition은 분리 history로 유지된다.
- 외부 task sync와 extension ingestion은 source projection을 재사용할 수
  있지만 각각 별도 계약이 필요하다.

## 검토한 대안

**A. `Suggestion`에 task state 추가.** 동결된 wire/storage enum을 변경하고
일시적 relevance feedback과 사용자 업무를 섞으며 network-generated durable
claim 위험을 만들기 때문에 기각했다.

**B. Todo를 먼저 저장하고 나중에 확인.** 생성을 곧바로 durable claim으로
만들고 erasure/retry 동작을 모호하게 하므로 기각했다.

**C. Receipt 없이 process-local single-flight 사용.** Restart, multi-window
call, commit-response crash를 견디지 못하므로 기각했다.

**D. Provenance를 위해 전체 OCR/extension envelope 저장.** Provenance는 source
content를 consent·retention 경계 밖까지 보존할 근거가 아니므로 기각했다.

**E. Timer로 interruption abandonment 추론.** 시간 경과는 사용자 의도의
증거가 아니며 expiry만 시간으로 파생할 수 있으므로 기각했다.

## 리뷰 및 구현 게이트

이 ADR을 `Accepted`로 바꾸기 전에 state machine, forbidden transition,
retention 기간, source-loss 동작, rollback 계약을 리뷰해야 한다. #8577 종료
전 다음 test를 포함해야 한다.

1. 모든 allowed/forbidden transition
2. duplicate/conflicting idempotency key
3. commit 전후 crash와 restart replay
4. 동일 delivery와 변경된 source revision의 dedupe
5. wall-clock rollback과 expiry reconciliation
6. consent revoke, source delete/access loss, export, full erasure
7. fresh DB, baseline upgrade, savepoint rollback, future-schema refusal
8. unknown forward-compatible source/state 값
9. crate-boundary와 IPC authority

## 알려진 후속 작업

1. **#8577 구현** — 본 ADR Accepted 후에만 구현한다.
2. **#8583 extension envelope** — 최소화 `WorkContextEnvelope` projection을
   `TaskSourceRef`로 매핑한다. Task table에 envelope 자체를 저장하지 않는다.
3. **Task sync ADR** — task table을 sync descriptor에 넣기 전에 conflict,
   tombstone, cross-device confirmation semantics를 정의한다.
4. **Reopen semantics** — 필요하면 terminal row 변경 없이
   `supersedes_todo_id` UX/history 표시를 정한다.

## 개정 2026-07-21: 리뷰 해소

본 ADR은 3-loop 적대적 리뷰(독립 devil's-advocate·rust-core 구현성·privacy/retention
렌즈) 후 `Proposed`→`Accepted`로 전환됐다. 리뷰가 4개 blocking 클러스터와 다수
important 갭을 드러냈으며 아래에서 모두 해소한다. 이 해소는 계약이며, 상충하는
기존 서술을 대체한다. 상태 머신(§2), confirmation 트랜잭션 형태(§3), `effective_now`
un-expiry 보호(§7), additive-migration 테스트 기준(§9)은 sound로 확인돼 불변이다.
정본은 영문 [Amendment 2026-07-21](./ADR-028-durable-task-lifecycle-boundary.md#amendment-2026-07-21-review-resolutions)이다.

### Blocking 해소

- **B1 — 결과코드 판별(§3).** CAS 실패 시: 요청 target 상태에 이미 도달했으면
  `already_transitioned`(다른 key로 원하는 종단 상태 달성), 현재 revision이 다르고
  target에 미도달이면 `revision_conflict`. `confirmed` 외 terminal(`dismissed`/`expired`)
  candidate에 대한 `confirm`은 그 terminal 상태를 담아 `already_transitioned`를 반환하며
  `revision_conflict`가 아니다. 호출자는 `already_transitioned`=idempotent no-op 성공,
  `revision_conflict`=refetch 후 재시도로 처리한다.
- **B2 — anchor 없는 source kind의 dedupe(§5).** upstream object/revision anchor가 없는
  source kind(특히 `local_current_scene`)는 `dedupe_namespace`에 per-capture occurrence
  식별자(capture/frame id 또는 단조 capture sequence)를 반드시 포함해, 서로 다른 두
  capture에서 나온 동일 sanitized 콘텐츠가 서로 다른 `dedupe_key`를 갖게 한다.
  content-hash-only dedupe는 단일 capture occurrence의 at-least-once 수집만 보호하며
  정당하게 재관측된 scene을 억제하지 않는다.
- **B3 / P1 — FK cascade·receipt 보존·dedupe tombstone(§6, §9).** 이 코드베이스는 SQLite
  `foreign_keys` PRAGMA를 켜지 않으므로(검증됨) 5개 테이블의 `ON DELETE`는 문서용이며
  엔진 캐스케이드가 아니다. 모든 content-clearing/tombstone expiry/erasure 삭제는
  `retention.rs`의 `ALL_TABLES` 패턴과 동일하게 트랜잭션 내부의 명시적 child-first
  `DELETE`로 애플리케이션 강제한다. 삭제가 애플리케이션 순서이므로
  `task_transition_receipts`와 dedupe tombstone은 부모 행과 독립적 보존 수명을 갖는다
  (부모 삭제가 이들을 암묵 삭제하지 않음 → receipt "+30일" 구현 가능). dismissed/expired
  candidate는 종단 트랜잭션에서 content를 지우되 최소 dedupe tombstone(`dedupe_key`,
  terminal outcome, timestamp — title/body/source text 없음)을 tombstone window 동안
  보존한다. window 내 retry는 terminal candidate를 반환(부활 없음)하고, window 경과·tombstone
  purge 후의 재전달은 새 proposal이다. Frozen Invariant #5는 tombstone window(v1=30일)로
  범위 한정된다.

### Important 해소

- **P2 — `task_source_refs` 삭제 시점(§6).** owning candidate 행과 함께 삭제:
  dismissed/expired candidate의 tombstone expiry 시, confirmed candidate의 provenance
  tombstone 해제 시(=originating to-do 삭제 시). 소유 candidate/to-do보다 오래 보존하지 않는다.
- **P3 — sanitizer floor(§6, 게이트 #10).** candidate title/body는 `PiiSanitizer` 포트로
  live capture 레벨(사용자가 `Off` 가능)과 독립적으로 최소 `PiiFilterLevel::Standard`에서
  sanitize하며(export의 `EXPORT_SANITIZE_LEVEL`과 동형), sanitizer 부재 시 fail-closed
  (candidate 미생성).
- **P4 — export masking 열(§6).** durable content 열(candidate/to-do `title`·`body`,
  dismiss `reason`)을 export `MASKED_COLUMNS`에 방어심층으로 추가. `body`·`reason`은
  아직 목록에 없어 #8577이 추가한다.
- **P5 / audit sink(§6).** 본 ADR은 task `title`/`body`/hash/source 식별자를 `audit_log`·
  `session_audit_log`·`egress_ledger`에 쓰지 않는다. §6의 "category·outcome only" 문장은
  금지 규정이며 기존 task→audit 경로 서술이 아니다.
- **I1 — manual to-do는 v1 범위 밖.** 모든 v1 to-do는 `TaskSourceRef`를 가진 candidate에서
  기원한다. source 없는 "Add Task"는 범위 밖이며, 도입 시 `manual` source_kind와 자체
  dedupe/provenance 규칙을 후속으로 정의한다. #8577은 가짜 source를 합성하지 않는다.
- **I2 — reopen은 미정(§2).** §2의 "future reopen flow"는 예시일 뿐이며 origin-candidate
  연결·`source_kind`는 Known Follow-up #4 소유다.
- **I3 — blocker edge 아이덴티티(§8, §9).** `entity_kind = todo_blocker`, idempotency는
  directed `(blocked_todo_id, blocker_todo_id)` 쌍으로 keying, `expected_revision`은 blocked
  to-do 기준 검사·증가(그 id가 receipt `entity_id`). blocker edge는 자체 ULID prefix 없음.
- **I4 — confirm 후 재전달(§5/§6).** 이미 confirmed된 source revision의 retry는 confirmed
  상태를 담은 `already_transitioned`를 반환(content-null "ghost" candidate 아님).
- **I5 — 부모 삭제 시 `todo_blockers`(§6/§9).** 애플리케이션 순서 삭제(B3)이므로 to-do
  명시 삭제 시 동일 트랜잭션에서 인접 blocker edge를 제거하고, blocker를 잃은 dependent
  to-do를 UI에 노출한다(무신호 unblock 금지).
- **I6 — `consent_required`/`source_unavailable` 트리거(§8).** 전자는 mutation 시점에 source
  category의 live consent 부재 시, 후자는 source lifecycle이 `active`가 아니고 live 수집이
  필요할 때. 둘 다 이미 confirmed된 to-do 작업에는 적용 안 됨(source 독립).
- **I7 — same-key race는 single-writer funnel에서 직렬화(§3).** idempotency는 크레이트의
  단일-writer SQLite funnel(`with_conn`/`with_conn_mut`, `parking_lot`)에 의존한다.
- **C1 — id-generation prefix 등록(구현 체크리스트).** #8577은 `id_generation.rs`의
  `USED_PREFIXES`에 `tcand`/`todo`/`tmut`를 등록한다(ADR-022 공개 registry는 이미 사전 등록).

## 관련 문서

- `docs/architecture/ADR-022-client-id-generation-ulid.md`
- `docs/architecture/ADR-026-async-storage-convergence-consent-port.md`
- `docs/architecture/ADR-027-suggestion-action-binding.md`
- `crates/maekon-storage/src/migration/mod.rs`
- `crates/maekon-storage/src/sqlite/maintenance/retention.rs`
- `crates/maekon-storage/src/sqlite/maintenance/export.rs`
