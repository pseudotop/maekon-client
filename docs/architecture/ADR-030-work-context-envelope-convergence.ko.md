[English](./ADR-030-work-context-envelope-convergence.md) | [한국어](./ADR-030-work-context-envelope-convergence.ko.md)

# ADR-030: Work Context Envelope와 수렴

**Status**: Accepted (2026-07-21 개정)
**Date**: 2026-07-19
**Scope**: `maekon-core` work-context 모델과 포트, 향후 암호화 ledger 어댑터, timeline/task/memory evidence projection
**Related**: ADR-022 (prefix+ULID ID), ADR-023 (memory evidence), ADR-026 (consent port), ADR-028 (durable task provenance), ADR-029 (Extension 경계), ADR-031 (확장 인가/계정 경계)
**Issue**: #8583 (`MK-EXT-01.S02`)

---

## 배경 (Context)

기존 `maekon_core::models::event::ContextEvent`는 foreground desktop
app/window 관측이다. 이 이벤트는 capture 결정을 공급하며 이미 PC event 계약의
일부로 직렬화·저장된다. 계약의 의미를 바꾸지 않고는 외부 provider의 account
identity, remote object version, access state, 삭제 또는 cursor replay를 표현할 수
없다.

외부 work system은 실제로 at-least-once source이기도 하다. crash 이후 같은 page가
두 번 전달될 수 있고, update가 순서 없이 도착할 수 있으며, 더 오래된 update가
replay되기 전에 object가 삭제될 수 있고, content revision 없이 access가 사라질 수
있다. 매 delivery를 새 event로 취급하면 timeline item이 중복되고 revoke 또는
retention expiry 이후 데이터가 되살아날 수 있다.

본 ADR은 connector나 SQLite schema를 구현하기 전에 별도
`WorkContextEnvelope`, acquisition port, 최소화 provenance, deterministic
convergence 규칙을 정의한다. durable task lifecycle을 다시 정의하거나 connector를
구축하거나 memory claim을 생성하거나 새 PC `Event` variant를 추가하지 않는다.

## 결정 (Decision)

### 1. 외부 work context는 별도 source family다

`WorkContextEnvelope`는 새 `maekon-core` work-context module에 속한다. wire
identifier는 `maekon.work_context_envelope.v1`이다. 로컬 envelope ID는 추가형
ADR-022 prefix `wctx`를 사용하며, 이 prefix는 본 ADR이 Accepted가 된 뒤에만
효력이 생긴다.

다음 계약은 동결한다:

- `ContextEvent`는 현재 app/window/activity field와 PC-capture 책임을 유지한다.
- `Event`에 `WorkContext` variant를 추가하지 않는다.
- 기존 PC event wire/storage, batch upload, capture trigger는 envelope를
  deserialize하거나 이에 따라 동작하지 않는다.
- 외부 context와 PC context는 명시적 source-family 구분을 가진 query/projection
  view에서만 만난다.

Envelope는 한 remote object에서 관측한 한 revision을 기록한다. 이는 ingestion의
evidence이며 provider가 정확히 한 번 전달했다거나 access가 지금도 존재한다는
증거가 아니다.

### 2. Envelope는 metadata와 provenance이며 raw payload가 아니다

필수 모델은 다음과 같다:

| 그룹 | 필드와 규칙 |
|---|---|
| local identity | `envelope_id`, `schema_version`, `access_epoch_id` |
| source identity | `extension_id`, `install_id`, opaque `account_subject_ref`, `remote_type`, `remote_id` |
| version | `revision_model`, optional `remote_revision`, optional `etag`, optional normalized source order, `content_hash` |
| classification | bounded `kind`, data classification, retention class |
| time | `occurred_at`, `source_updated_at`, `observed_at`, `ingested_at` |
| relations | optional opaque thread, parent, project, actor ref |
| authority evidence | 최소화한 access snapshot과 consent snapshot. 어느 것도 live authority가 아님 |
| provenance | ingest-run ID, 이전 accepted envelope ID, source cursor/page digest, projection/raw-blob ref |
| lifecycle | `active`, `deleted`, `access_revoked`, `retention_expired` 중 하나 |

Source ref는 이름, email, token이 포함된 URL, raw ACL entry가 아닌 opaque
identifier다. Relation ref에는 bounded ref kind, opaque source ID, optional
fingerprint만 포함한다. Envelope에는 message body, document text, meeting note,
attachment, HTML, provider payload, OAuth token, ACL member list 또는 search token을
포함하지 않는다.

`account_subject_ref`는 이 이슈의 `account_id` 요구사항을 privacy-minimized하게
표현한 값이다. 선언된 extension과 installation 경계 안에서만 stable하며 display
name이나 email address를 account ID로 사용하지 않는다.

Bounded kind taxonomy는 다음과 같다:

- `message`
- `meeting`
- `document`
- `issue`
- `decision`
- `task`
- `unknown`

알 수 없는 source kind는 inventory와 export에서 round-trip해야 하지만, 명시적으로
mapping하기 전에는 searchable projection, suggestion input, task 생성 또는 graph
projection에 사용할 수 없다.

Data classification은 `public`, `internal`, `confidential`, `restricted`,
`unknown`이며, `unknown`은 `restricted`로 강제한다. Retention class는 local policy
engine이 해석하는 policy identifier이지 provider가 공급한 TTL이 아니다.

### 3. 시간 필드는 서로 다른 의미를 가진다

| 필드 | 의미 | ordering authority |
|---|---|---|
| `occurred_at` | business event 발생 시점. optional/provider-authored | version authority로 사용하지 않음 |
| `source_updated_at` | provider가 보고한 마지막 수정 시점. optional | 해당 connector 계약이 monotonic semantics를 증명한 경우에만 비교 가능 |
| `observed_at` | connector가 remote record를 관측한 local time | 진단용이며 remote conflict resolution에 사용하지 않음 |
| `ingested_at` | envelope transaction이 commit된 local time | local audit/retention 시작점이며 remote version authority가 아님 |

로컬 ledger는 clock rollback 중에도 local operation을 monotonic하게 만들기 위해 기존
HLC로 write를 stamping할 수 있다. 이 HLC는 remote revision인 척하지 않는다.
Provider-specific 계약이 timestamp를 authoritative하다고 명시하지 않는 한 wall-clock
timestamp로 opaque remote-version tie를 깨지 않는다.

### 4. Identity와 revision dedupe는 canonical하고 account-scoped다

Source object identity는 다음 필드의 canonical length-prefixed encoding이다:

```text
extension_id || install_id || account_subject_ref || remote_type || remote_id
```

역산할 수 없는 local key는 다음과 같다:

```text
source_object_key = HMAC-SHA256(local-dedupe-key, "work-context-object/v1\0" || identity)
```

HMAC은 보관된 key 또는 public export가 provider/account ID dictionary로 변하는 것을
막는다. Provider가 동일한 remote ID를 재사용하더라도 서로 다른 account 또는
installation은 충돌할 수 없다.

각 record에는 revision fingerprint도 부여한다:

```text
SHA256(
  "work-context-revision/v1\0" || revision_model ||
  remote_revision? || etag? || source_updated_at? ||
  content_hash || lifecycle
)
```

Canonical encoding은 length prefix와 명시적인 missing-value marker를 사용한다.
Local uniqueness key는 `(source_object_key, access_epoch_id,
revision_fingerprint)`다. 이를 replay하면 원래 ingest result를 반환하고 envelope나
projection을 추가 생성하지 않는다.

`content_hash`는 ciphertext, raw ACL 또는 provider JSON byte가 아니라 canonical
sanitized content를 대상으로 한다. Source cursor는 account/install-scoped
checkpoint이며 object identity, version authority 또는 cross-account dedupe key가
아니다.

제품은 at-least-once acquisition 아래 idempotent local upsert를 약속한다. exactly-once
remote delivery를 주장하지 않는다.

### 5. 모든 connector는 revision 품질을 선언한다

`ContextSourceDescriptor`는 다음 revision model 중 하나를 선언한다:

| 모델 | 계약 |
|---|---|
| `monotonic` | provider semantics가 한 object에 대해 total order를 보장하는 경우에만 connector가 normalized order value를 제공 |
| `opaque` | revision/etag는 equality만 지원하며 변경된 값끼리는 비교 불가 |
| `content_hash_only` | provider에 신뢰할 수 있는 version token이 없음. active update는 sanitized content hash로 dedupe하고 production 지원에는 입증된 delete strategy 필요 |

Adapter는 provider version을 normalize할 수 있지만 provider evidence 없이 lexical
string 또는 timestamp에서 monotonic ordering을 주장해서는 안 된다. Delete/access
loss를 deterministic하게 처리할 수 없는 connector는 capability `unavailable`을
보고하며 supported로 광고할 수 없다.

### 6. Merge와 lifecycle 규칙은 deterministic하고 fail closed한다

한 `access_epoch_id` 안에서 lifecycle은 monotonic하다:

```text
active ──source delete────> deleted
   ├────access loss───────> access_revoked
   └────retention expiry──> retention_expired
```

Terminal state는 content availability를 제거한다. Re-authentication/re-grant는 새
access epoch를 만들며, 기존 tombstone을 active로 되돌리지 않는다.

한 source object와 access epoch의 merge는 다음 순서로 진행한다:

1. 동일한 revision fingerprint는 `duplicate`이며 저장된 result를 replay한다.
2. Access/consent denial은 즉시 `access_revoked`를 만들고 content를 지우며 content
   revision보다 우선한다.
3. 비교 가능한 더 높은 active revision은 이전 active projection을 교체한다.
4. 비교 가능한 더 낮은 revision은 `stale`이며 교체하거나 되살릴 수 없다.
5. 동일한 비교 가능 revision에서 content hash가 다르면 `revision_conflict`다.
6. Delete/retention tombstone은 같거나 낮은 모든 비교 가능 active revision을
   suppress한다.
7. 변경된 opaque revision은 `incomparable`이다. 정렬된 revision fingerprint로
   계산한 deterministic conflict ID 아래 metadata를 보존하지만 search, suggestion,
   task 또는 graph projection에는 winner를 노출하지 않는다.
8. `deleted` 이후 active record는 provider 계약이 undelete를 명시적으로 지원하고
   strictly higher comparable revision을 제공할 때만 수용한다. Access revoke에는 항상
   새 epoch가 필요하다.

따라서 delete-before-update는 안전하다. Tombstone이 replay된 오래된 update를
차단한다. Delete와 update가 비교 불가능하면 object를 conflict-quarantine한 채 더
안전한 tombstone을 visibility에 적용한다. Delivery order는 어떤 content가
searchable해지는지 결정하지 않는다.

Local `retention_expired`는 같거나 오래된 revision을 suppress한다. 실제로 더 새로운
revision은 현재 consent/access가 있고 policy가 acquisition을 계속 허용할 때만 ingest할
수 있다.

### 7. Raw, projection, envelope, tombstone plane을 분리한다

| Plane | Content | 암호화와 retention |
|---|---|---|
| raw payload blob | bounded parsing 또는 reprocessing에 필요한 provider response/body | 기본 memory-only. 명시적 consent가 있으면 account/install-scoped key로 AEAD 암호화, 기본 TTL 24h, hard maximum 7d |
| searchable projection | timeline/search에 필요한 sanitized title/body/summary와 bounded ref | at-rest 암호화. 기본 TTL 30d이며 user/source retention을 넘지 않음 |
| envelope | 최소화한 identity, version, classification, provenance, hash, lifecycle | identifier가 남는 곳은 암호화. projection 또는 확인된 reference obligation과 함께 보존 |
| suppression tombstone | HMAC source key, access epoch, version/order fingerprint, lifecycle, deletion time | content-free. `max(provider replay horizon, projection retention, 90d)` 동안 보존, hard maximum 365d |

Connector는 bounded replay horizon을 선언해야 한다. 안전한 suppression에 365일을
초과하는 기간이 필요하거나 horizon을 알 수 없으면 다른 convergence design이
Accepted될 때까지 해당 connector를 unsupported로 유지한다.

Raw와 projection의 key, TTL, export path, erasure job은 독립적이다. Raw blob 삭제는
최소화 envelope를 삭제하지 않는다. Source 삭제/revoke는 raw와 projection content
availability를 즉시 제거한다. Content-free tombstone은 replay resurrection을 막는
목적으로만 남을 수 있다.

### 8. Access와 consent snapshot은 evidence이며 authority가 아니다

`AccessSnapshot`은 access decision(`allowed`, `denied`, `unknown`), visibility
class, non-reversible scope fingerprint, evaluation time, provider-policy version을
포함한다. Raw ACL member list는 포함하지 않는다. Connector가 raw ACL을 일시
처리해야 한다면 raw TTL 아래 encrypted raw plane에만 두고 public DTO에는 절대
직렬화하지 않는다.

`ConsentSnapshot`은 product permission ID/version, decision, evaluation time을
포함한다. Consent token이나 mutable grant object를 포함하지 않는다.

모든 timeline query, search, suggestion input, projection read, source-open request는
live product consent, account access, source lifecycle, retention을 재평가한다. 저장된
snapshot은 이후 read를 authorize할 수 없다. `unknown` access는 deny한다.
Revoke/access loss는 in-flight page work를 취소하고 cursor advance를 막으며 같은
durable operation에서 content를 지우고 cached DTO를 사용할 수 없게 한다.

### 9. Port는 acquisition과 persistence 및 public delivery를 분리한다

`maekon-core`는 다음 두 개의 좁고 object-safe한 async port를 정의한다:

- `ContextSourcePort`는 source/revision/delete capability를 기술하고 한 opaque
  account subject와 cursor에 대해 bounded `ContextSourcePage`를 pull한다.
- `WorkContextStorePort`는 idempotent ingest, convergence, projection, tombstone,
  cursor, export, erasure operation을 수행한다.

Page에는 page/checkpoint digest, record, optional next cursor, `has_more`가 포함된다.
Record는 최소화 source/version/access metadata와 in-memory 또는 sealed raw-payload
handle을 전달한다. Handle은 public DTO가 아니며 adapter가 storage를 직접 write하지
않는다.

Application use case는 live consent/access check를 수행하고 page를 atomic하게
commit한다:

1. source descriptor와 account epoch 검증
2. identity와 revision canonicalize
3. 각 deterministic merge result 적용
4. 허용되는 projection write/replace 및 raw expiry schedule
5. tombstone/conflict receipt write
6. account/install cursor advance
7. commit 이후에만 event emit

Commit 전에 crash가 발생하면 cursor가 advance하지 않는다. Commit은 성공했지만
provider response/ack가 유실되면 page가 replay되고 local uniqueness가 이전 result를
반환한다. Process-local single-flight는 최적화일 뿐이다.

### 10. ADR-028 `TaskSourceRef`는 최소화한 immutable projection이다

| `TaskSourceRef` 필드 | Envelope mapping |
|---|---|
| `source_kind` | `extension_context` |
| `extension_id` / `install_id` / `account_subject_ref` | 동일한 opaque source identity field |
| `upstream_object_id` | access/retention이 허용하는 동안 remote object ID. 이후 ADR-028에 따라 제거 |
| `upstream_revision` / `upstream_etag` | 허용되는 동안 accepted source version |
| `occurred_at` / `observed_at` | 동일한 timestamp 의미 |
| `dedupe_namespace` | raw identity tuple가 아닌 HMAC source-object key에서 파생한 task-specific namespace |
| `content_hash` | accepted sanitized projection hash |
| `lifecycle` | `active`, `deleted`, `access_revoked`, `retention_expired`로 mapping |
| `source_outcome` | 없음. interruption source용으로 reserved |

Confirmation은 이 최소화 ref를 copy하며 task provenance에 envelope, raw blob, ACL
snapshot 또는 projection을 저장하지 않는다. 이후 source loss는 사용자가 확인한
to-do를 지우지 않지만 source-open은 fail closed하고 UI는 lifecycle reason을
표시한다. Task 계약의 소유권은 ADR-028에 그대로 있다.

### 11. Timeline과 memory는 복사한 source content가 아닌 evidence ref를 사용한다

Query layer는 PC event와 external projection을 명시적 display-time policy로 정렬한
`TimelineEvidenceItem` view로 결합할 수 있다. 모든 item은
`source_family = pc_event | work_context`를 유지하며 serialization conversion이나
inheritance는 없다. External envelope는 screen capture를 trigger하지 않는다.

Memory graph와 suggestion pipeline은 envelope ID, accepted revision fingerprint,
classification, lifecycle, content hash를 담은 `WorkContextEvidenceRef`를 받는다.
Consent/access gate를 통해 live sanitized projection을 dereference할 수 있다. Raw
provider content를 claim, node, prompt 또는 audit에 복사하지 않는다. Memory graph
generation policy의 소유권은 #8087에 그대로 있다.

### 12. Export, revoke, erasure, source-open을 명시한다

Personal-data export는 기존 masking path를 통해 envelope, projection, conflict,
cursor, tombstone 설명을 포함한다. 아직 보관 중인 raw blob은 재인증하고 명시적으로
선택한 encrypted attachment path로만 export하며 normal public JSON DTO에는 나타나지
않는다. Credential과 raw ACL member list는 절대 export하지 않는다.

Consent revoke 또는 account/source access loss는 다음을 수행한다:

- acquisition과 cursor advance를 중단한다.
- bounded suppression에 불필요한 raw/projection content와 account/remote identifier를
  제거한다.
- replay horizon 동안 HMAC-keyed content-free tombstone만 보존한다.
- timeline/search/suggestion/task-source-open cache를 무효화한다.

Full local erasure는 먼저 connector를 disable하고 credential/cursor를 제거한 뒤,
active acquisition path가 없어 resurrection 가능성이 사라지므로 suppression
tombstone까지 모든 envelope plane을 삭제한다. 향후 relay 또는 cross-device
propagation에는 별도 erasure-convergence ADR이 필요하다.

확인된 to-do는 ADR-028에 따라 source erasure 후에도 남을 수 있지만 “open source”는
`source_unavailable`을 반환한다. Stale URL을 따라가거나 credential을 refresh하거나
hash로 content를 복원하지 않는다.

### 13. Forward compatibility는 읽을 수 있고 mutation-safe해야 한다

알 수 없는 kind/classification/lifecycle/revision 값은 inventory와 export에서
round-trip한다. Projection, suggestion, task 생성 또는 source-open에는 사용할 수
없다. 필수 security/source identity field가 없으면 validation에 실패한다.

최소 typed result는 다음을 포함한다:

- `ingested`
- `duplicate`
- `stale_revision`
- `revision_conflict`
- `revision_incomparable`
- `source_deleted`
- `access_revoked`
- `consent_required`
- `retention_expired`
- `projection_unavailable`
- `source_unavailable`
- `cursor_replayed`

Public DTO는 opaque account/source ref와 bounded access/lifecycle state를 노출한다.
Account secret, raw ACL member, raw payload handle, provider token 또는 masking하지 않은
remote URL은 절대 노출하지 않는다.

## 동결 불변식 (Frozen Invariants)

다음 항목을 변경하려면 새 ADR 또는 명시적 update가 필요하다:

1. PC `Event`/`ContextEvent` wire, storage, capture semantics는 분리된 상태를 유지한다.
2. Local ingestion은 at-least-once delivery에서 idempotent하며 exactly-once remote를
   주장하지 않는다.
3. Account/install identity는 모든 object, cursor, grant, dedupe boundary에
   참여한다.
4. Access revoke는 content revision보다 우선하며 새 access epoch가 필요하다.
5. 비교 불가능하거나 충돌한 revision은 projection, suggestion, task 또는 memory
   graph를 공급하지 않는다.
6. Raw payload, projection, envelope, tombstone은 서로 다른 encryption, TTL, export,
   erasure 동작을 가진다.
7. 저장된 access/consent snapshot은 live read를 authorize하지 않는다.
8. Task와 memory는 복사한 raw source data가 아니라 최소화한 evidence ref를 보존한다.

## 결과 (Consequences)

### 긍정적

- Connector 구현 전에 cursor replay와 out-of-order delivery가 deterministic하다.
- 기존 PC event/capture 계약이 안정적으로 유지된다.
- Revoke/delete/retention은 stale page replay로 되돌릴 수 없다.
- Task, timeline, memory consumer가 source payload를 복사하지 않고 provenance를
  공유한다.

### 부정적

- Opaque provider revision은 가능성이 높은 winner를 고르는 대신 content를
  quarantine할 수 있다.
- 네 storage plane과 account-scoped cursor에는 더 많은 lifecycle job과 property
  test가 필요하다.
- Bounded replay/delete model이 없는 provider는 supported로 광고할 수 없다.

### 중립적

- HLC는 local monotonic audit/order에 재사용하지만 provider revision semantics를
  대체하지 않는다.
- SQLite schema와 connector-specific version normalization은 이 계약 안에서 수행할
  후속 구현 결정으로 남는다.

## 검토한 대안 (Alternatives Considered)

**A. `ContextEvent` 확장.** External object는 desktop foreground observation이 아니며
event wire, storage, capture trigger를 바꾸므로 기각했다.

**B. Cursor 또는 delivery time을 object identity/version으로 사용.** Cursor는
replay되고 restart 이후 delivery order가 달라지므로 기각했다.

**C. Opaque conflict에 last-write-arrival 사용.** Delivery order에 따라 서로 다른
content가 노출되고 deleted data가 되살아날 수 있으므로 기각했다.

**D. Provenance를 위해 complete provider JSON과 ACL 보관.** Provenance는 source
content나 access list의 장기 보관을 정당화하지 않으므로 기각했다.

**E. External text를 timeline/task/memory에 복사.** 복사된 content가 source access,
consent, retention revoke 경계 밖으로 빠져나가므로 기각했다.

**F. Revoke에서 모든 tombstone을 즉시 삭제.** Cursor 또는 connector가 완전히
disable되기 전 at-least-once page replay가 삭제된 projection을 다시 만들 수 있으므로
기각했다.

## 개정 2026-07-21 (승인 검토, Amendment)

3-loop 적대적 검토(프라이버시 렌즈 + devil's-advocate 렌즈)에서 `Proposed` 본문에
대해 blocking 결함 3건, important 공백 6건, minor 명확화 4건이 제기되었다. 아래에서
각각을 해소한다. 이 해소 사항은 원래의 결정(Decision) 절과 동일한 구속력을 갖는다.
Status를 `Accepted`로 전환한다.

### B1 — envelope 평면에 확정 보존 상한을 부여한다 (§7)

§7은 나머지 세 평면에는 숫자 상한을 부여했으나(raw 기본 24시간 / 하드 최대 7일,
projection 30일, tombstone 최소 90일 / 하드 최대 365일), envelope 평면은 "투영 또는
확정 참조 의무와 함께 보존"이라고만 기술했다. 살아 있는 투영도 없고 확정된 ADR-028
참조도 없는 envelope — 한 번 조회되었고 확정되지 않았으며 투영은 이미 만료된 객체 —
는 결국 만료 시점이 전혀 없었고, 이는 4-평면 분리가 존재하는 이유인 보존 논거 자체를
무력화한다.

해소: envelope은 다음을 초과해 존속해서는 안 된다.

    max(해당 객체의 투영 보존기간, 확정된 ADR-028 참조 수명)

두 의무가 모두 없으면 투영 기본값(30일)을 상한으로 승계하며, tombstone 평면과 동일한
하드 최대 365일을 적용한다. envelope 만료는 원격 삭제가 아니다. 객체는
`retention_expired`로 전이하고 내용 없는 억제 tombstone만 남으며, 그 tombstone은 자체
독립 상한을 유지한다. 보존 잡(job) 속성 테스트는 투영도 확정 참조도 없는 envelope
사례를 반드시 포함해야 한다.

### B2 — `local-dedupe-key`의 보관 방식을 가정하지 않고 명시한다 (§4)

§4는 `source_object_key = HMAC-SHA256(local-dedupe-key, ...)`의 비가역성에 의존하여,
보존된 키나 공개 내보내기가 공급자/계정 ID 사전(dictionary)이 되지 않는다고 주장한다.
그러나 그 키가 어디에 저장되고 어떻게 생성되는지는 전혀 기술하지 않았다. ADR-031은
자신의 `local-account-key`에 대해 동일한 질문을 Blocking Resolution으로 해소해야 했고,
현재 두 키는 모두 ADR 본문에만 존재한다. `crates/maekon-storage/src/keychain.rs`와
`device_identity.rs` 어디에도 구현자가 참고할 선례가 없다.

해소(ADR-031 §1과 동형):

- `local-dedupe-key`는 설치당 1회 CSPRNG로 생성하고, `KeychainRegistry`를 통해
  Keychain 전용으로 저장하며, SQLite·설정 파일·로그·텔레메트리·내보내기 어디에도
  기록하지 않는다.
- `device_identity.device_id` 또는 이미 평문으로 영속화되었거나 서버로 전송되는 다른
  값에서 파생해서는 안 된다. 그렇게 하면 HMAC의 존재 이유인 비가역성 보장이 붕괴한다.
- ADR-031의 `local-account-key`와는 별개의 비밀이다. 설치당 단일 루트 비밀을 선호하는
  경우 두 키를 도메인 분리된 서브키로 둘 수 있으나
  (`HKDF-SHA256(root, info = "maekon.dedupe-key.v1")`,
  `HKDF-SHA256(root, info = "maekon.account-key.v1")`), 동일한 키 재료를 두 이름으로
  사용해서는 안 된다.
- 키 분실은 치명적이지 않고 복구 가능한 사건이다. 중복 제거가 "전부 새 것으로 보임"
  수준으로 저하되고 원장은 리비전 기준으로 재수렴한다. 키 분실은 어떤 경우에도 키 없는
  해시로 후퇴할 사유가 되지 않는다.

### B3 — "전체 로컬 소거"와 ADR-031의 제거(uninstall)는 서로 다른 동작이다 (§12)

§12는 전체 로컬 소거가 "먼저 커넥터를 비활성화하고 … 억제 tombstone을 포함한 모든
envelope 평면을 삭제한다"고 기술한 반면, ADR-031 §11(Accepted)은 uninstall이 계정
연결 해제 순서를 반복하며 그 5단계는 최소화된 provenance와 억제 tombstone을 *보존*한다고
규정한다. 문자 그대로 읽으면 두 문서는 확장 제거에 대해 정반대 지시를 준다.

해소: 두 동작은 서로 구별되는 사용자 동작이며, uninstall은 ADR-031 §11이 규율한다.

| 동작 | 억제 tombstone | 근거 |
|---|---|---|
| 계정 연결 해제 / 확장 제거 (ADR-031 §11) | 재생(replay) 지평 동안 **보존** | 계정이 재연결될 수 있고, 재전송된 낡은 페이지는 여전히 억제되어야 한다 |
| 전체 로컬 소거 (본 ADR §12) | **삭제** | 자격증명·커서·커넥터까지 파괴하는, 사용자가 명시적으로 시작하는 별도의 "모든 로컬 데이터 삭제" 경로이며, 이후 어떤 수집 경로도 데이터를 되살릴 수 없다 |

§12의 "커넥터를 비활성화한다"는 소거 경로 내부의 순서를 기술한 것이지 uninstall을
가리키지 않는다. 전체 로컬 소거는 확장 제거나 계정 연결 해제의 부수 효과로 도달
가능해서는 안 된다.

### I1 — 평면 키는 기존 루트 키의 HKDF 서브키다 (§7)

§7은 raw 평면 블롭을 "계정/설치 범위 키"로 AEAD 암호화하라고 요구했으나 그 키의 근거를
제시하지 않았다. 현재 존재하는 유일한 키 인프라는
`crates/maekon-storage/src/encryption/mod.rs`의 단일 전체 DB
`EncryptionKey([u8; 32])`(AES-256-GCM)이며, `file_transport.rs`의 패스프레이즈 기반
`derive_key`는 내보내기 파일용 메커니즘이지 평면 키가 아니다.

해소: 평면 키는 기존 `EncryptionKey`의 HKDF-SHA256 서브키로 한다. 새로운 키 관리 체계를
도입하지 않는다.

    raw 평면 키 = HKDF-SHA256(ikm  = EncryptionKey,
                              salt = install_id,
                              info = "maekon.raw-plane.v1" || account_subject_ref)

한 계정의 저장된 salt/컨텍스트 레코드를 파괴하면 DB를 재작성하지 않고도 해당 계정의 raw
평면을 암호적으로 파쇄(crypto-shred)할 수 있으며, 이것이 철회 시 §12의 "raw 내용을
제거한다"를 충족하는 승인된 메커니즘이다. 구현은 #8589 소관이다.

### I2 — `access_epoch_id`는 저장소가 발급하며 브로커의 카운터가 아니다 (§6, §9)

§6은 재승인이 "새 접근 에포크를 생성한다"고 했으나, 나열된 `WorkContextStorePort`
오퍼레이션 중 에포크를 발급하는 동사가 없고, ADR-031의 개정은 자기 쪽에서만 카운터 비공유를
명확화했다. ADR-030만 읽는 독자는 두 카운터가 존재한다는 사실조차 알 수 없었다.

해소: `access_epoch_id`는 work-context 저장소가 소유하고 발급한다. 능력 브로커도, 커넥터도
아니다. `WorkContextStorePort`에 명시적 `begin_access_epoch(account)` 오퍼레이션을 추가하며,
최초 인가 시점과 모든 재승인 시점에 호출된다. 브로커는 철회/재승인을 신호할 뿐 에포크 값을
공급하지 않는다. 한 번에 수집된 페이지의 모든 레코드는 동일한 에포크를 지녀야 하고, 계정의
현재 에포크와 일치하지 않는 페이지는 커서를 전진시키지 않고 폐기한다. ADR-031의 브로커
취소 에포크는 본 카운터와 순서 관계가 없는 별개의 카운터다.

### I3 — `account_subject_ref`는 ADR-031의 `account_id`다 (§2)

§2는 `account_subject_ref`를 "이슈의 account_id 요구사항을 프라이버시 최소화하여 표현한
것"이라고만 기술했고 공식이 없었다. 또한 ADR-030의 관련 문서 목록에서 ADR-031이 통째로
누락되어 있었다 — 정작 ADR-031은 "세 번째 계정 정체성 표현을 발명하는 것"을 명시적으로
경고하고 있는데도 그렇다.

해소: `account_subject_ref`는 ADR-031의 `account_id` **그 자체**다. 동일한 문자열을 그대로
전달하며, 독립적으로 재유도하지 않고 다시 해싱하지도 않는다. 아래 관련 문서에 ADR-031을
추가한다.

### I4 — 커서 전진은 compare-and-swap이다 (§9)

§9의 7단계 페이지 커밋은 단일 호출에서는 올바르지만, 한 계정에 대해 겹치는 두 수집(재시작이
진행 중인 페치와 경합하거나, 스케줄러가 재발화)이 각각 커서 C0를 읽고 각자의 후속 커서를
기록하여 나중 기록이 앞선 기록을 덮어쓰거나 후퇴시키는 상황을 막지 못했다. 내용 수준 중복
제거가 데이터 손실은 막지만, 불필요한 재페치나 두 루프가 서로의 커서를 되돌리는 상황은 막지
못한다.

해소(ADR-028의 `expected_revision`과 동형): 커서 기록은 해당 페이지 시작 시점에 읽은 커서
값에 대한 compare-and-swap이다. 불일치 시 페이지를 폐기하고 — 아무것도 커밋하지 않으며 커서도
전진시키지 않는다 — 다음 예정 실행이 현재 커서를 다시 읽는다. 계정별 수집 직렬화를 추가로
구현할 수 있으나, CAS는 그와 무관하게 필수다.

### I5 — 시계 되돌림이 평면 TTL을 연장할 수 없다 (§3, §7)

§3은 HLC 스탬핑을 선택적인 로컬 편의로만 언급했고, 병합 알고리즘은 이를 참조하지 않았으며 네
평면 TTL 어디에도 되돌림 방지 장치가 없었다. 이 TTL들이 4-평면 설계의 프라이버시 논거 전부인
만큼, 되돌려진 시스템 시계는 raw 블롭을 무기한 "아직 만료되지 않음" 상태로 유지할 수 있었다.

해소(ADR-028 §7과 동형): 네 평면 모두 만료를 다음 기준으로 평가한다.

    effective_now = max(current_utc, persisted_last_ingested_at)

따라서 시계를 뒤로 돌리면 데이터가 더 일찍 만료될 수는 있어도 더 늦게 만료될 수는 없다.
`ingested_at`은 계속 로컬 보존 기준점이며(§3), 원격 타임스탬프는 TTL 평가에 관여하지 않는다.

### I6 — 목록에서의 부재는 삭제 증거가 아니다 (§5)

`content_hash_only` 커넥터에서 가장 자연스러운 삭제 전략은 "객체가 목록에 더 이상 나타나지
않음"이지만, 이는 at-least-once 페이지네이션 전달에서 건전하지 않다. 페이지 유실, 순서 뒤바뀜,
일시적 공급자 오류가 살아 있는 객체를 부재한 것처럼 보이게 만든다.

해소: 어떤 리비전 품질 등급에서도 목록 부재를 삭제 증거로 취급해서는 안 된다. `deleted`
생명주기 전이는 명시적 공급자 삭제 신호(이벤트, 웹훅, 감사 항목, 또는 객체 범위 페치의 확정적
not-found)를 요구한다. 그러한 신호가 없으면 객체는 로컬 보존이 만료시켜 `retention_expired`가
될 때까지 `active`로 유지되며, 이것이 클라이언트가 실제로 아는 바에 대한 정직한 진술이다.

### 사소한 명확화 (Minor)

- **M1 (§6 규칙 7)** — 충돌 격리 메타데이터는 전달 건당 1건이 아니라
  `(source_object_key, access_epoch_id)`당 1건이며, B1에서 확정한 envelope 평면 상한을 따른다.
- **M2 (§4, ADR-022)** — `wctx` 프리픽스는 현재
  `crates/maekon-core/src/id_generation.rs`의 `USED_PREFIXES`에 **없다**(2026-07-21 실측;
  ADR-028의 `tcand`/`tmut`/`todo`는 존재). 등록은 #8587의 작업 범위이며 기존 사실이 아니다.
- **M3 (§4)** — 설치된 두 확장 또는 두 계정이 동일한 실물 원격 객체를 각각 별도의 work-context
  레코드로 노출할 수 있다. 이는 의도된 동작이며 ADR-028 §5의 자동 병합 금지 원칙을 따른다.
  클라이언트는 두 공급자 객체가 동일한 것이라고 결코 단정하지 않는다.
- **M4 (§4, §12)** — `content_hash`는 ADR-028에서 승계하여 계속 salt가 없다. 로컬 범위
  내에서는 허용되지만, 제3자가 설치 간 상관관계를 추정할 수 있는 어떤 형태로도 내보내거나
  전송해서는 안 된다. #8589가 네 평면을 구현할 때 `PERSONAL_DATA_EXPORT_TABLES`와
  `MASKED_COLUMNS`를 동일 PR에서 확장해야 한다. 두 곳 모두에서 누락된 신규 테이블은 GDPR
  내보내기와 마스킹을 조용히 깨뜨린다.

## 검토 및 구현 게이트 (Review and Implementation Gates)

본 ADR이 `Accepted`가 되기 전에 reviewer는 identity encoding, revision model,
conflict quarantine, lifecycle dominance, retention bound, TaskSourceRef mapping,
full-erasure 동작을 승인해야 한다. 이후 구현 property test는 다음을 포함해야 한다:

1. duplicate page 및 duplicate item
2. stale 및 higher comparable revision
3. content가 다른 equal revision
4. changed opaque revision/incomparable conflict
5. 두 delivery order 모두의 delete-before-update 및 update-before-delete
6. page 처리 중 access revoke 및 새 epoch를 가진 re-grant
7. multi-account same remote ID와 cursor 분리
8. commit 전 crash, commit 후/ack 전 crash, restart replay
9. raw/projection TTL 독립성과 clock rollback
10. source delete, retention expiry, export, revoke, full erasure
11. unknown forward-compatible value와 public DTO secret/ACL 제외
12. PC Event/capture wire non-regression

## 알려진 후속 작업 (Known Follow-ups)

1. **#8584 permission/OAuth 계약** — access epoch가 사용하는 account credential과
   dynamic grant reconciliation 정의
2. **#8587 source runtime** — Wave-0 계약이 Accepted된 뒤에만 page scheduling 구현
3. **#8589 encrypted ledger** — task lifecycle 또는 memory generation policy를
   추가하지 않고 네 plane, query projection, retention job 구현
4. **#8087 memory graph policy** — live evidence ref가 memory claim이 될 수 있는지와
   그 방식을 결정
5. **Relay/cross-device ADR** — envelope 또는 erasure tombstone이 local device를
   벗어나기 전에 필요

## 관련 문서 (Related Docs)

- `docs/architecture/ADR-022-client-id-generation-ulid.md`
- `docs/architecture/ADR-023-local-symbolic-memory-graph.md`
- `docs/architecture/ADR-028-durable-task-lifecycle-boundary.md`
- `docs/architecture/ADR-029-extension-package-runtime-boundary.md`
- `docs/architecture/ADR-031-extension-authorization-account-boundary.md`
- `crates/maekon-core/src/models/event.rs`
- `crates/maekon-core/src/models/sync.rs`
- `crates/maekon-storage/src/sync_retention_tombstone.rs`
