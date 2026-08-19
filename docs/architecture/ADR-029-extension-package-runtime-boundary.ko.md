[English](./ADR-029-extension-package-runtime-boundary.md) | [한국어](./ADR-029-extension-package-runtime-boundary.ko.md)

# ADR-029: Extension Package와 Runtime 경계

**상태**: Accepted (2026-07-21 3-loop 리뷰 후 개정 — 정본 영문 ADR-029 Amendment 2026-07-21 참조)
**일자**: 2026-07-19
**범위**: `maekon-core` extension 계약, `maekon-automation` trust 재사용, `src-tauri` composition root, extension-facing UI/API
**관련**: ADR-001 (crate 방향), ADR-002 (실행 경계), ADR-019 (typed error), ADR-026 (consent port), ADR-028 (영속 task provenance)
**이슈**: #8585 (`MK-EXT-01.S01`)
**구현 상태**: Registry P01 #8586은 parent main `75d6a1e9af`에서 SQLite V50·core/store·lifecycle IPC·독립 panel까지 구현됐고, Skill Pack activation #8588은 `4b80bf4bdf`에서 SQLite V52·activation IPC로 구현됐다. Panel은 여전히 미장착이고 Skill Pack은 frontend 표면이 없으며, connector·marketplace·third-party runtime은 미구현이다. Source-only readback이며 release·runtime·고객 효과 증거가 아니다.

---

## 배경

Maekon에는 인접 자산이 있지만 하나의 installable Extension 계약은 없다.

- `AutomationTemplatePackageManifest`는 재사용 automation template의 publisher,
  signature, install/execute 승인, evidence, rollback, dry-run, egress trust를
  정의한다.
- `PermissionProfileV2`는 filesystem, network, Unix socket, resource limit를
  정의한다.
- managed policy는 user config를 clamp하며 consent를 대신할 수 없다.
- `FileSkillLoader`는 local Markdown instruction을 읽지만 registry, installer,
  stable public API가 아니다.
- Provider Surface Catalog는 AI provider readiness를 소유하며 installable
  package를 소유하지 않는다.
- read-only MCP 작업은 Maekon이 다른 process에 context를 제공하는 방향이며
  Extension을 load하는 방향과 반대다.

이 중 하나를 Extension runtime으로 일반화하면 다른 trust boundary를 복제하거나
약화한다. 임의 native code loading은 package trust, revoke, compatibility가
검증되기 전에 desktop process와 local data를 third-party execution host로 만든다.

이 ADR은 product vocabulary, package taxonomy, runtime disposition, lifecycle
truth, threat boundary를 정의한다. Registry, connector, marketplace, relay,
executable plugin loader는 구현하지 않는다. `Proposed`는 리뷰 가능하지만 아직
효력이 없다는 뜻이다.

## 결정

### 1. Product term은 각각 하나의 책임을 가진다

| 용어 | 책임 | 해당하지 않는 것 |
|---|---|---|
| **Extension** | manifest, trust identity, 선언된 contribution을 가진 versioned install/update/uninstall 단위 | 임의 executable 또는 marketplace listing |
| **Context Source Connector** | 외부 업무 context를 읽어 bounded source contract를 만드는 read-only adapter | action executor, Skill Pack, raw-payload archive |
| **Skill Pack** | capability dependency를 선언한 verified instruction/data bundle | native code, consent grant, direct tool authority |
| **Action Adapter** | 별도 action 계약과 매회 사람의 승인 뒤 외부 system에 쓰는 미래 adapter | Phase-1 contribution 또는 context sync의 암묵적 side effect |
| **ONESHIM Relay** | webhook continuity와 organization policy를 위한 선택적 미래 execution location | 필수 Maekon backend, Extension type, silent fallback |

Extension은 packaging/lifecycle이다. Contribution은 package 안의 capability-bearing
요소다. Runtime은 contribution을 평가하는 mechanism이다. 이 개념들을 하나의
overloaded `type` 또는 `connected` flag로 표현하지 않는다.

### 2. Manifest는 별도 versioned core 계약이다

Public wire identifier는 `maekon.extension_manifest.v1`이다. Model은
`maekon-core`에 둔다. Adapter는 artifact를 검증할 수 있지만 trust/capability
field를 만들어내면 안 된다.

필수 shape는 다음과 같다.

| Group | 필수 field |
|---|---|
| identity | `extension_id`, semantic `version`, immutable `publisher_id`, `package_digest` |
| source trust | bounded `source_kind`, signature state/key ID 또는 `app_bundle` trust, signed manifest digest |
| compatibility | `manifest_schema`, inclusive `host_api_min`, exclusive `host_api_max` |
| runtime | `runtime_kind`, `execution_location`, public DTO에 filesystem path를 노출하지 않는 entry-point identifier |
| contributions | stable contribution ID, bounded kind, contribution API version, requested capabilities |
| policy | 선택적 `permission_profile_id`, external-egress 선언, data classification, retention class |
| lifecycle | update channel, rollback window, minimum allowed version, uninstall cleanup 선언 |

`extension_id`는 reverse-DNS style이며 version 간 stable하다. Installation에는
별도 local opaque `install_id`를 발급한다. Account identity는 package identity의
일부가 아니며 installation 아래 opaque account subject로 표현한다. Manifest에는
OAuth secret, account token, raw ACL member list, cursor, payload, user path,
executable command line을 넣지 않는다.

Phase 1은 Extension당 정확히 하나의 `context_source` contribution만 받는다.
Reserved taxonomy는 다음과 같다.

| Contribution kind | Code/data trust | Phase-1 disposition |
|---|---|---|
| `context_source` | adapter code는 first-party이며 signed app에 compile되어야 함; remote payload는 항상 untrusted data | supported contract, read-only only |
| `skill_pack` | signed/curated data와 instruction; capability를 grant하거나 source content가 되지 않음 | capability-resolver ADR 전 unavailable |
| `action_adapter` | executable external-write boundary | 별도 action/HITL ADR 전 unavailable |
| unknown future value | unknown | inventory/export용 보존; enable/operation unavailable |

여러 contribution을 한 package에 묶는 일은 defer한다. 무관한 grant를 결합해
confused-deputy 경로를 만들 수 있으므로 후속 ADR이 필요하다.

### 3. Phase 1은 built-in read-only runtime만 지원한다

Runtime disposition은 normative하다.

| Runtime kind | Disposition | 이유 |
|---|---|---|
| `first_party_builtin` | **Phase 1 지원**, read-only `context_source`만 | code가 app trust/signing boundary 안에서 배포됨; manifest는 설명만 하며 load하지 않음 |
| `declarative` | unavailable | bounded schema, interpreter, resource limit, golden compatibility test 필요 |
| `mcp` | unavailable | MCP server consume/install은 기존 read-only provider 방향과 다름 |
| `subprocess` | unavailable | dedicated protocol, binary provenance, OS sandbox, kill/timeout, audit 계약 필요 |
| `wasm` | unavailable | engine supply-chain review, host-call allowlist, fuel/memory, deterministic ABI 필요 |
| arbitrary native binary | rejected | 통제되지 않은 executable supply chain |
| dynamic library (`dylib`/DLL) | rejected | trusted process 안에서 실행해 process isolation 우회 |
| in-process Node/Rust plugin | rejected | same-process code execution은 product trust boundary 밖 |

Package source disposition도 같은 수준으로 좁다.

| Source kind | Phase-1 disposition |
|---|---|
| `app_bundle` | manifest digest가 signed app release에 결합된 first-party built-in만 지원 |
| `local_file` | unavailable; local path는 publisher/signature trust가 아님 |
| `curated_registry` | signed distribution/update와 key-rotation 계약 전 unavailable |
| `public_marketplace` | unavailable; listing, download, rating, install, support claim 없음 |
| unknown | inventory/export only; unavailable |

“Supported runtime”은 특정 provider가 shipped/proven됐다는 뜻이 아니다. 개별
connector는 auth, sync, restart, revoke, delete, rate-limit evidence를 통과한
뒤에만 support 가능하다. 그 전에는 UI/API가 typed unavailable reason을
반환한다.

### 4. Existing E74 asset은 ownership을 바꾸지 않고 재사용한다

| Existing asset | Disposition | Extension rule |
|---|---|---|
| `AutomationTemplatePackageManifest` / #7441 | **semantic 재사용; 별도 schema로 확장** | publisher/source/signature, install-vs-execution approval, evidence, egress, dry-run, rollback 의미 재사용; Extension으로 deserialize 금지 |
| `PermissionProfileV2` / #7437 | **재사용** | manifest가 profile/capability를 request할 수 있으나 live resolver가 authoritative; deny precedence 약화 금지 |
| managed policy / #7438, #4832 | **재사용** | runtime/source/capability/provider/version/egress를 좁힐 수 있음; user consent/OAuth scope 생성 금지 |
| record-to-template / #7442 | **defer** | authoring을 재구현하거나 observed content를 Skill Pack으로 승격하지 않음 |
| `FileSkillLoader` | **defer** | internal Markdown discovery는 registry/install trust가 아니며 기존 파일 auto-migration 금지 |
| Provider Surface Catalog | **일반화 거부** | AI-provider readiness owner 유지; Extension Registry는 별도 bounded catalog |
| read-only MCP provider / #7440, #7919 | **분리 유지** | sanitized context 제공은 Extension runtime이 아니며 MCP server install 금지 |
| sandbox worker | **재사용 defer** | 미래 subprocess가 OS-isolation 원칙을 재사용할 수 있으나 현재 worker는 plugin loader가 아님 |

`automation.template_package_trust.v1`은 지금처럼 읽고 검증한다. 자동 manifest
migration, wrapper install, registry discovery는 없다. 나중에 공통 Rust value
object를 추출하더라도 두 wire format은 byte/field compatible해야 하며 golden
test로 보호한다. 기존 automation template은 conversion으로 Extension
capability를 얻지 않는다.

### 5. Lifecycle과 readiness는 서로 직교하는 state axis다

Registry/API/UI는 다음 axis를 분리해 보고한다.

| Axis | 최소 값 |
|---|---|
| availability | `available`, `unavailable(reason)` |
| installation | `bundled`, `not_installed`, `installing`, `installed`, `install_failed`, `uninstalling`, `uninstalled` |
| enablement | `disabled`, `enabled` |
| account authentication | `not_required`, `unauthenticated`, `authenticating`, `authenticated`, `revoked`, `auth_error` |
| capability grant | account/capability별 `not_requested`, `pending`, `granted`, `denied`, `revoked`, `expired` |
| operation | `sync`, `query` 등의 operation kind와 `idle`, `running`, `blocked(reason)`, `failed(reason)` |
| update | `current`, `update_available`, `staged`, `activating`, `update_failed`, `rollback_available` |
| health | `unknown`, `healthy`, `degraded(reason)`, `unhealthy(reason)` |

Install permission, enablement, provider authentication, product consent,
capability grant, operation approval은 서로 다른 사실이다.

- installed는 enabled가 아니다.
- enabled는 authenticated가 아니다.
- OAuth scope는 Maekon product consent가 아니다.
- authentication은 capability grant가 아니다.
- read grant는 external-write approval이 아니다.
- managed allowance는 user approval이 아니다.

“Connected” 같은 UI summary는 모든 필수 axis가 healthy일 때만 파생할 수 있고
위 axis로 펼쳐 보여야 한다. Unsupported, revoked, stale, partial configuration을
숨기면 안 된다.

### 6. Effective capability는 사용 시점에 평가하는 intersection이다

Account-scoped operation `O`에 대해:

```text
effective(O) =
  product consent
  ∩ manifest request
  ∩ installation grant
  ∩ provider/account scope
  ∩ managed policy
  ∩ runtime readiness
  ∩ current source health
```

모든 항은 정확한 operation, resource class, account subject, egress target을
허용해야 한다. Missing, stale, unknown, 이해 범위보다 broad한 값은 deny한다.
Check는 sync page, retry, projection read, 미래 action마다 실행한다. Install-time
check만으로는 부족하다.

Package는 self-grant할 수 없다. Manifest capability 추가, account/publisher/
signature-key/runtime 변경, broadened egress set은 관련 grant를 무효화하고 새
review를 요구한다. Managed policy는 intersection을 deny하거나 더 좁힐 수 있지만
user-denied 항을 allow로 바꿀 수 없다.

### 7. Standalone local operation이 success path다

`execution_location` reserved 값은 다음과 같다.

| 값 | 의미 |
|---|---|
| `local` | 사용자 Maekon device에서 실행하며 ONESHIM 불필요 |
| `relay` | explicit configured ONESHIM Relay를 거치는 미래 operation; Phase 1 unavailable |
| `either` | 미래 package가 양쪽을 지원하지만 active location은 explicit; silent local-to-relay failover 금지 |

Phase 1은 `local`만 받는다. Relay outage가 supported standalone connector를
차단하면 안 된다. Organization managed policy는 location을 deny할 수 있지만
data를 relay로 조용히 이동할 수 없다. Relay auth, retention, webhook continuity,
tenant isolation, organization authorization은 별도 ADR/evidence가 필요하다.

### 8. Update, rollback, uninstall은 trust를 보존하고 data를 지운다

Update는 activation 전에 stage한다. Registry는 schema/API compatibility,
package digest, trusted publisher/signature continuity, minimum allowed version,
runtime disposition, capability diff를 검증한다. Activation은 version을
atomically swap한다. 실패하면 마지막 verified version을 유지하고
`update_failed`를 보고하며 partial staged package를 실행하지 않는다.

Rollback은 explicit하며 managed/security minimum보다 낮지 않은 retained,
previously verified artifact만 선택한다. Rollback은 revoked grant, credential,
provider scope, consent, erased data를 복원하지 않는다. Capability difference는
activation 시 다시 평가한다.

Uninstall 순서는 다음과 같고 fail-closed다.

1. 새 operation을 disable하고 in-flight work를 cancel/timeout한다.
2. local capability grant를 revoke한다.
3. OS Keychain에서 account-scoped credential을 삭제한다.
4. Connector retention 계약에 따라 cursor, raw payload cache, searchable
   projection content, retry material을 erase한다.
5. source provenance를 상황에 맞게 `deleted`, `access_revoked`,
   `retention_expired`로 표시한다.
6. cleanup receipt가 durable해진 뒤 package/runtime metadata를 제거한다.

사용자가 confirmed한 Todo는 조용히 삭제하지 않는다. ADR-028은 source loss 뒤
최소화 source provenance만 남긴다. Uninstall failure는 visible/retryable하며,
credential/content cleanup이 끝나지 않았는데 `uninstalled`를 보고하면 안 된다.

### 9. Threat model은 normative하며 bounded하다

| Threat | 필수 mitigation |
|---|---|
| supply-chain substitution | built-in app-bundle trust; 미래 artifact digest/signature/publisher/source/version 검증; staged activation과 audit |
| downgrade/rollback attack | managed minimum version, verified retained artifact only, grant 복원 금지 |
| indirect prompt injection | 모든 external payload는 untrusted data; system/developer instruction, Skill Pack, capability declaration, tool authority로 승격 금지 |
| credential theft | OS Keychain, account/install-scoped namespace, opaque handle; manifest/config/log/telemetry/public DTO secret 금지 |
| confused deputy | call-time exact account/resource/operation capability intersection; package self-grant/cross-account cursor reuse 금지 |
| stale permission/revoke race | page/retry마다 live check, cancellation, cursor quarantine, fail-closed projection read |
| cross-account confusion | identity에 extension/install/account subject 포함; cursor/cache/provenance/grant account scope 분리 |
| untrusted runtime escape | Phase 1 loaded runtime 없음; 미래 subprocess/WASM은 별도 sandbox/ABI ADR 필요 |
| malicious update broadening | capability/publisher/runtime/egress diff가 grant 무효화; explicit activation 전 old version 유지 |
| relay data expansion | Phase 1 relay unavailable; silent execution-location fallback 금지 |

External content는 후속 envelope, ledger, consent, content-guard 계약을 통해서만
summary/projection할 수 있다. Package signing은 remote content를 trusted
instruction으로 만들지 않는다.

### 10. Compatibility와 unavailability는 fail-closed다

Host는 manifest schema major를 이해하고
`host_api_min <= current_host_api < host_api_max`일 때만 manifest를 받는다.
Empty/inverted range, unknown runtime/contribution/capability, missing security
field, invalid digest/signature, publisher mismatch, unsupported location은 typed
unavailable result를 반환하며 best-effort execution을 시도하지 않는다.

최소 reason code는 다음을 포함한다.

- `manifest_schema_unsupported`
- `host_api_incompatible`
- `runtime_unsupported`
- `contribution_unsupported`
- `source_unsupported`
- `marketplace_unavailable`
- `publisher_untrusted`
- `signature_unverified`
- `managed_policy_denied`
- `consent_required`
- `authentication_required`
- `capability_grant_required`
- `source_unhealthy`
- `cleanup_incomplete`

Unknown non-security display metadata는 forward compatibility를 위해 round-trip할
수 있다. Unknown security/lifecycle/capability semantics는 inventory/export
data로만 남긴다. UI/API는 같은 canonical resolver를 사용하고 unavailable
package를 installed, connected, supported라고 표시하지 않는다.

### 11. Public export에는 계약을 넣고 민감 evidence는 넣지 않는다

본 ADR, 미래 manifest schema, bounded reason code, lifecycle semantics, high-level
threat category는 public-export 가능하다. Contributor가 claim/fail-closed behavior를
검증할 수 있게 한다.

Parent-only material은 internal strategy comparison, vendor selection note,
red-team payload corpus, exploit reproduction detail, secret/key location,
unpublished provider identifier, operational incident evidence를 포함한다. Public
docs에는 mitigation/test obligation을 쓸 수 있지만 해당 artifact를 복사하지
않는다. 본 ADR 승인만으로 marketplace, third-party runtime, relay, provider
support를 주장하면 안 된다.

## 동결 불변식

다음 항목 변경은 새 ADR 또는 명시적 update가 필요하다.

1. Phase 1 runtime은 first-party, built-in, local, read-only `context_source`만
   지원한다.
2. 임의 native binary, dynamic library, in-process Node/Rust plugin은 없다.
3. Install, enable, authenticate, grant, operation, update, uninstall은 별도
   state fact로 유지한다.
4. Effective capability는 live intersection이며 managed policy는 user consent를
   대신하지 않는다.
5. External payload는 untrusted data이며 Skill Pack instruction이 되지 않는다.
6. 기존 automation template, Provider Surface Catalog, MCP provider 계약은 기존
   ownership/wire 의미를 유지한다.
7. Standalone local success는 ONESHIM을 요구하지 않는다.
8. Unsupported runtime/contribution은 unavailable로 보고하며 simulate/silent
   degradation하지 않는다.

## 결과

### 긍정

- 첫 connector proof가 작고 audit 가능한 runtime surface를 가진다.
- 기존 trust/permission asset을 wire contract 혼동 없이 재사용한다.
- UI/API readiness가 partial setup과 unsupported runtime을 정직하게 표시한다.
- Payload storage 구현 전 uninstall/revoke/source-loss 의무가 생긴다.

### 부정

- Phase 1은 third-party connector 또는 임의 community plugin을 설치할 수 없다.
- 분리 lifecycle axis와 capability diff는 단일 enabled flag보다 registry/UI
  구현 비용이 크다.
- Declarative, subprocess, WASM, Skill Pack, Action Adapter, relay는 각각 추가
  설계/evidence가 필요하다.

### 중립

- Built-in connector code가 app과 함께 배포돼도 lifecycle honesty를 위해
  Extension으로 표현한다.
- 별도 bridge 승인 전 existing automation template은 Extension Registry 밖에
  남는다.

## 검토한 대안

**A. `AutomationTemplatePackageManifest`를 제자리 일반화.** Automation execution
trust는 connector identity/auth/sync lifecycle이 아니며 wire 변경이 existing
contract를 불안정하게 하므로 기각했다.

**B. `FileSkillLoader` directory를 installed Extension으로 취급.** Local file
discovery에는 publisher, signature, compatibility, permission, update, uninstall
보장이 없어 기각했다.

**C. Native/Node/Rust plugin을 process 안에서 load.** Extension 하나가 desktop
process의 data/authority를 얻고 process isolation을 우회하므로 기각했다.

**D. MCP를 universal plugin runtime으로 사용.** Existing MCP 방향은 read-only
Maekon provider이며 MCP server install/consume은 별도 executable/credential
boundary이므로 기각했다.

**E. Connector에 ONESHIM Relay를 필수화.** Standalone product invariant를 깨고
egress/availability scope를 조용히 넓히므로 기각했다.

**F. `connected` state 하나만 보고.** Install/auth/grant/consent/health/operation
중 실제로 무엇이 없는지 숨기므로 기각했다.

## 리뷰 및 구현 게이트

본 ADR을 `Accepted`로 바꾸기 전에 E74 reuse table, runtime disposition,
effective-capability formula, lifecycle axis, uninstall order, public/private
evidence split을 승인해야 한다. 후속 구현은 다음을 test해야 한다.

1. unknown schema/runtime/contribution/capability fail-closed
2. install vs enable vs auth vs grant state truth
3. 모든 capability-intersection 항과 revoke race
4. manifest compatibility/digest/signature/publisher/version check
5. update capability diff, atomic activation, failed update, rollback floor
6. multi-account cursor/grant/cache separation
7. uninstall cleanup success, partial failure, restart, retry
8. untrusted content의 instruction/capability 비승격
9. typed UI/API unavailability parity
10. parent-only threat evidence의 public-export 제외

## 구현 상태와 알려진 후속 작업

1. **#8583 envelope 계약** — external source identity, revision, tombstone,
   minimized task provenance를 정의한다.
2. **#8584 permission/OAuth 계약** — account-scoped credential, dynamic grant,
   provider scope reconciliation을 정의한다.
3. **#8586 registry P01 구현 완료·미장착** — parent main `75d6a1e9af`에서
   lifecycle axis, SQLite V50, IPC, frontend hook과 독립
   `ExtensionRegistryPanel`·component test가 merge됐다. 그 panel은 어떤 route
   항목에도, 테스트가 아닌 어떤 소스 파일에도 등장하지 않아 extension
   lifecycle에 도달 가능한 UI가 없다. `4b80bf4bdf`가 Skill Pack
   catalog/activation과 capability resolver(SQLite V52,
   `activate_skill_pack`·`clear_skill_pack_activation` IPC, 회귀 수정
   `26aa185c82`)를 추가했으나 frontend 표면은 전혀 없다. App·route composition과
   실제 connector/runtime은 별도 후속 범위다.
4. **Subprocess/WASM ADR** — non-built-in runtime enable 전에 필요하다.
5. **Action Adapter ADR** — external write contribution 전에 필요하다.
6. **Relay ADR** — `relay`/`either` available 전 필요하다.

## 개정 2026-07-21: 리뷰 해소

3-loop 적대적 리뷰(devils-advocate + rust-core) 후 Proposed→Accepted. 정본은 영문
[Amendment 2026-07-21](./ADR-029-extension-package-runtime-boundary.md#amendment-2026-07-21-review-resolutions).
핵심 해소(계약):
- **B1**: `bundled`(코드 출처)와 `installation`(설치 상태)는 별개 축 — 번들 built-in도
  not_installed→installed 전이. 두 컬럼으로 모델링, 병합 금지.
- **B2**: 정상 동작 중 로그/텔레메트리/DTO에 시크릿 미노출을 검증하는 11번째 테스트 게이트 추가.
- I1 publisher 변경=hard reject, I2 `execution_location_unsupported` 코드 추가,
  I3 update/uninstall 경합 규칙, I4 health reason 코드, I5 extension reason 코드=별도 네임스페이스.
- **어휘/P01(#8586)**: §5의 8 raw axes만으로 self-contained(ADR-031·ADR-030 불필요),
  단일 라벨은 ADR-031 §5 canonical 사용. 신규 `ExtensionManifest`(frozen automation 확장 금지),
  `install_id` 프리픽스 등록, 다음 마이그레이션 v50.

## 관련 문서

- `docs/architecture/ADR-001-rust-client-architecture-patterns.md`
- `docs/architecture/ADR-002-os-gui-interaction-boundary.md`
- `docs/architecture/ADR-026-async-storage-convergence-consent-port.md`
- `docs/architecture/ADR-028-durable-task-lifecycle-boundary.md`
- `crates/maekon-automation/src/template_package.rs`
- `crates/maekon-core/src/config/sections/privacy.rs`
- `crates/maekon-core/src/config/managed.rs`
- `crates/maekon-core/src/ports/mcp_readonly.rs`
