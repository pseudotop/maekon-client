[English](./ADR-031-extension-authorization-account-boundary.md) | [한국어](./ADR-031-extension-authorization-account-boundary.ko.md)

# ADR-031: Extension Authorization과 Account 경계

**상태**: Accepted (2026-07-21 3-loop 리뷰 후 개정 — 정본 영문 ADR-031 Amendment 2026-07-21 참조)
**Date**: 2026-07-19
**Scope**: `maekon-core` Extension authorization 모델과 포트, 향후 account-scoped OAuth/Keychain 어댑터, runtime capability broker, readiness UI/API
**Related**: ADR-008 (token refresh), ADR-021 (consent), ADR-026 (consent port), ADR-029 (Extension 경계), ADR-030 (work-context 수렴)
**Issue**: #8584 (`MK-EXT-01.S03`)

---

## 배경 (Context)

#5713의 기존 standalone provider 경로와 #8080의 integration lifecycle은 유용한
OAuth, OS-Keychain, readiness, revoke primitive를 제공한다. 그러나 Extension
installation의 account, capability grant, provider scope, cursor ownership, dynamic
revoke semantics는 정의하지 않는다.

특히 현재 `OAuthPort`와 `OAuthClient`는 `provider_id`를 key로 사용한다. 따라서 token
namespace와 refresh lock은 같은 provider의 여러 account를 여러 Extension
installation에서 사용하는 경계가 아니라 하나의 managed AI-provider connection을
기술한다. 이 key를 Extension account 경계로 재사용하면 credential이 섞이고 한
account의 scope 또는 refresh result가 다른 account에 적용될 수 있다.

OAuth authorization은 product consent도 아니다. Provider가 넓은 scope를 grant해도
Maekon consent, package manifest, installation grant 또는 managed policy는 더 좁은
operation만 허용할 수 있다. Installed, enabled, authorized, granted, ready를 하나의
`connected` boolean으로 줄이는 UI는 backend가 허용되거나 사용할 준비가 되기 전에
sync control을 노출할 수 있다.

본 ADR은 provider connector, scheduler, storage schema를 구현하기 전에 dynamic
Extension authorization 계약을 정의한다. 현재 managed-AI OAuth port를 주장만으로
일반화하거나 provider endpoint를 구현하거나 어떤 Extension도 available하게 만들지
않는다.

## 결정 (Decision)

### 1. Authorization에는 account-scoped canonical identity가 있다

Canonical `ExtensionAccountKey`는 다음을 포함한다:

```text
extension_id || install_id || provider_id || account_id
```

`account_id`는 issuer-qualified provider subject와, 해당하는 경우 tenant/workspace
identity에서 파생한 opaque local account subject다. Display name, email address,
login hint, access token 또는 provider URL이 아니다. Provider가 email이 아닌 stable
subject를 제공하지 않으면 multi-account 지원은
`unavailable(account_identity_unstable)`로 유지한다.

Local value는 역산할 수 없고 installation-bound다:

```text
account_id = base32(HMAC-SHA256(
  local-account-key,
  "extension-account/v1\0" || issuer || provider_subject || tenant_or_workspace?
))
```

Canonical encoding은 length prefix와 명시적 missing-value marker를 사용한다.

한 issuer, tenant, Extension 또는 installation의 account subject는 다른 경계에서
같은 text를 가져도 같지 않다. Cursor, secret, grant, health, rate limit, cache,
projection, provenance, refresh lock, in-flight work key는 모두 완전한
`ExtensionAccountKey`를 포함한다.

논리적 OS-Keychain namespace는 다음과 같다:

```text
extension/{extension_id}/{install_id}/{account_id}
```

모든 path segment를 validate하고 길이를 제한한다. `account_id`는 위의 opaque local
subject이므로 Keychain inventory가 email 또는 tenant directory가 될 수 없다.
Provider와 tenant identity는 value metadata와 namespace derivation에 bind하며,
namespace lookup은 opaque credential handle을 반환하기 전에 둘 다 검증해야 한다.

### 2. 기존 primitive는 입증된 경계 안에서만 재사용한다

| 기존 자산 | 처분 | Extension 규칙 |
|---|---|---|
| `ConsentManagerPort` / `effective_permissions` | 재사용 | live product-consent term. withdrawal과 erasure가 계속 authoritative함 |
| `OAuthPort` / `OAuthClient` | key shape가 아닌 구현 pattern 재사용 | PKCE, callback cancellation, token classification, redirect-disabled credential POST, refresh coordination은 새 adapter에 참고. provider-only API는 multi-account Extension authorization을 충족하지 않음 |
| `SecretStore` / `KeychainOps` | adapter 재사용 | secret value는 OS-Keychain에만 존재. Extension namespace는 account/install scoped이고 enumeration metadata는 privacy-sensitive함 |
| `InternalRangePolicy`와 hardened outbound client | 재사용 | remote authorization/token/revoke/introspection endpoint에는 본 ADR의 strict policy 적용 |
| provider readiness snapshot | vocabulary 재사용 | Extension readiness는 별도 account/install snapshot이며 AI-provider availability를 상속하지 않음 |
| ADR-029 lifecycle/grant axes | authoritative | 본 ADR은 이 axis를 축약하지 않고 authentication, capability, operation, health, revoke 동작을 구체화함 |
| ADR-030 access epoch와 envelope lifecycle | authoritative | account authorization은 access epoch를 만들고 종료함. work-context erase/tombstone 동작은 ADR-030 소유 |

기존 OAuth code는 현재 AI-provider 계약을 계속 제공할 수 있다. Additive
account-scoped port와 adapter가 본 ADR의 matrix를 통과하기 전에는 Extension
multi-account 지원으로 광고해서는 안 된다.

### 3. Effective capability는 정확한 live 교집합이다

Operation `O`, resource class `R`, account `A`에 대해:

```text
effective(A, O, R) =
  global Maekon consent
  ∩ manifest declaration
  ∩ installation capability grant
  ∩ account OAuth scope
  ∩ managed policy
  ∩ OS and provider readiness
```

각 term은 정확한 Extension, installation, account, capability, operation, resource
class, egress target, current policy version에 대해 평가한다. Missing, stale, unknown,
expired, partially understood 또는 host가 mapping할 수 있는 범위보다 넓은 값은
deny한다. Deny가 항상 우선한다.

Managed policy는 provider, capability, resource, egress target 또는 version을 제거할
수 있다. Product consent를 만들거나 manifest capability, installation grant, OAuth
scope를 추가하거나 user denial을 allow로 바꿀 수 없다.

External write 또는 destructive action은 교집합이 성공한 뒤에도 별도 per-run 또는
per-action approval이 필요하다. 이 approval은 single-purpose, short-lived,
account-bound이며 install approval, OAuth authorization 또는 향후 action grant로
재사용할 수 없다.

### 4. Approval과 authorization fact를 분리한다

다음은 서로 독립된 record이며 각각 별도로 revoke된다:

| Fact | 허용하는 것 | 허용하지 않는 것 |
|---|---|---|
| install approval | 하나의 installation에 verified package 등록 | enablement, account access, content collection, execution |
| enablement | host가 installation을 고려하도록 허용 | OAuth, capability, sync, action |
| capability grant | installation과 optional account에 대한 하나의 bounded capability/resource/egress set | provider scope, product consent, destructive action |
| OAuth authorization | 한 account에 provider가 발급한 scope | Maekon capture/OCR/full-text consent 또는 package capability |
| per-run/action approval | preview와 expiry를 가진 하나의 exact external action | future run, background sync, 새 account, 더 넓은 target |

Capability, resource, account mode, provider, runtime 또는 egress를 넓히는 manifest
변경은 영향받는 grant만 invalidate하고 readiness를 non-ready state로 되돌린다. Silent
scope escalation과 automatic reauthorization은 금지한다.

### 5. Lifecycle summary는 boolean이 아니라 orthogonal state에서 파생한다

Authoritative snapshot은 ADR-029의 installation, enablement, authentication, grant,
operation, update, availability, health axis에 current product consent, scope delta,
Keychain readiness, policy version, account identity verification을 더해 보존한다.
Summary는 한 installation, account, required capability set에 대해 평가하며, 하나의
capability 상실이 관련 없는 account 또는 capability를 revoke하지 않는다. 필수 product
summary는 다음 값으로 파생한다:

| Summary | 파생과 허용 transition |
|---|---|
| `discovered` | verified descriptor를 알지만 설치하지 않음 |
| `installed` | 설치했지만 disabled. operation 불가 |
| `enabled` | enabled지만 consent/grant/account/auth/readiness term 중 하나 이상 미완료 |
| `authorizing` | 명시적 account-bound OAuth flow 하나가 active |
| `ready` | 필요한 모든 live intersection term이 allow하고 active operation 없음 |
| `syncing` | fresh decision 아래 한 authorized account operation이 active |
| `degraded` | bounded transient provider, rate-limit, network 또는 backend health failure 존재. denied operation은 계속 deny |
| `stale` | token, scope, account, grant, policy 또는 readiness evidence가 오래되거나 변경되어 재검증 필요 |
| `revoked` | product consent, install grant, OAuth authority, account access 또는 policy authority가 명시적으로 상실됨 |
| `incompatible` | manifest/runtime/API/provider 계약을 host가 안전하게 이해할 수 없음 |

정상 진행은 `discovered -> installed -> enabled -> authorizing -> ready -> syncing ->
ready`다. Transient failure는 `ready` 또는 `syncing`을 `degraded`로 이동시킬 수 있고,
입증된 recovery는 full intersection을 다시 평가한 뒤 `ready`로 돌아간다. 변경되거나
불확실한 evidence는 optimistic `ready`가 아니라 `stale`로 이동한다. 명시적 authority
loss는 모든 active state에서 `revoked`로 이동하며 해당하는 새 authorization/grant가
필요하다. Host incompatibility는 항상 operational summary보다 우선한다.

Summary는 display와 filtering용이다. Consumer는 underlying axis, denial reason,
freshness, remediation key를 받는다. `connected`는 `ready` 또는 `syncing`의 localized
copy로만 표시할 수 있고 저장된 source of truth가 아니다.

### 6. OAuth는 minimum scope로 시작하고 명시적으로 upgrade한다

각 connector는 versioned capability-to-scope map을 선언한다. 첫 authorization은
사용자가 방금 선택한 capability의 minimum scope만 요청한다. Identity scope는 별도
정당화가 필요하며 offline access는 bounded background operation에 refresh가 필요한
경우에만 요청한다.

Callback 이후 adapter는 provider가 반환한 normalized scope set을 기록하고 requested
set 및 이전 granted set과 비교한다:

- 필수 scope가 빠지면 영향받는 capability만 deny하고 `scope_partial`을 보고한다.
- 예상하지 않은 extra provider scope는 Maekon capability를 넓히지 않는다.
- scope response가 없으면 provider 계약이 authoritative introspection path를 제공하지
  않는 한 `scope_unverified`다.
- 더 많은 scope가 필요한 capability는 delta와 목적을 보여주는 명시적 incremental
  upgrade preview를 trigger한다.
- upgrade 거절은 이전에 valid한 narrower capability를 보존하며 관련 없는 account를
  disconnect하지 않는다.

Incremental flow는 같은 `ExtensionAccountKey`, install grant, manifest digest,
requested scope delta, redirect, state, PKCE challenge, short expiry에 bind한다.
Callback의 account가 다르면 flow가 실패하고 token을 저장하지 않는다.

다른 교집합 term이 독립적으로 allow하지 않는 한 OAuth scope는 screen capture, OCR,
full-text retention, suggestion input, task 생성, external write 또는 다른 Maekon
product permission을 절대 grant할 수 없다.

### 7. OAuth endpoint와 redirect는 pin하고 fail closed한다

Provider authorization metadata는 host-curated이거나 signed connector descriptor에
대해 검증한다. Phase 1에서는 user-supplied authorization, token, refresh, revoke,
introspection, issuer 또는 redirect endpoint를 받지 않는다.

Adapter는 다음을 강제한다:

1. 모든 remote endpoint의 exact HTTPS scheme, normalized host, 허용 port, issuer,
   path
2. connector에 등록한 loopback callback 또는 승인된 OS application link만 포함하는
   fixed redirect allowlist
3. `127.0.0.1`/`::1` loopback binding, exact random flow path 또는 one-time state를
   더한 fixed registered path, 최대 5분 lifetime, wildcard redirect 금지
4. constant time으로 비교하고 account, install, manifest, scope request, redirect에
   bind한 high-entropy one-time state
5. flow마다 fresh verifier를 쓰는 PKCE S256, `plain` downgrade 금지
6. token, refresh, revoke, introspection request의 redirect following 비활성화. 3xx는
   typed failure이며 credential body를 replay하지 않음
7. 모든 remote destination에서 loopback, private, link-local, metadata, CGNAT, NAT64,
   mapped IPv4, multicast, unspecified 및 resolved internal address를 strict reject
8. 승인된 public address를 DNS rebinding으로 교체할 수 없도록 resolve-and-pin 또는
   connect-time address verification
9. 모든 authorization backend request의 bounded response size와 timeout

Local callback만 loopback 예외이며 provider response body를 받지 않는다. Browser는
validated authorization URL만 열 수 있다. 반환 link, `Location` value, provider error
URL, remediation URL은 새 allowlist check 없이 backend가 절대 따라가지 않는다.

### 8. Token과 provider body는 secret boundary를 넘지 않는다

OS Keychain은 access, refresh, ID token, token-set generation 및 모든 provider
credential material의 authoritative store다. Manifest, config, SQLite, cursor,
projection, cache, log, audit, telemetry, crash report, clipboard, public DTO, UI state,
error text에는 token, authorization code, PKCE verifier, state value, client secret,
cookie 또는 raw credential handle이 없다.

Non-secret authorization metadata에는 opaque account ID, provider ID, normalized
granted-scope name 또는 scope fingerprint, expiry bucket, credential generation, issuer
fingerprint, policy/grant version, typed health, timestamp만 포함할 수 있다. Keychain
enumeration metadata는 owner-only이며 privacy-sensitive하고 value를 포함하지 않는다.

Provider response body, redirect target, query string, header, raw error description은
log/audit/telemetry에 절대 넣지 않는다. Adapter는 capped body를 memory에서 읽고
allowlisted typed OAuth error code만 추출한 다음 body를 폐기한다. Public error는 stable
reason code, retryability, bounded status class, localization/remediation key만 노출한다.

Provider `error_description`을 log하는 현재 code는 이 Extension 계약 준수 evidence가
아니다. Raw description을 callback HTML에 render하는 경우도 마찬가지다. 향후
adapter는 사용 전에 두 path를 모두 redact하거나 교체해야 한다.

### 9. Refresh는 account-scoped, rotation-safe, deterministic하다

Refresh single-flight key는 완전한 `ExtensionAccountKey`와 token-set generation을
사용한다. 서로 다른 account는 독립적으로 refresh한다. 같은 account의 concurrent
refresh는 stale token을 readiness evidence로 반환하지 않고 committed result를
await하거나 replay한다.

성공한 refresh는 token generation을 atomic compare-and-swap한다. Provider가 refresh
token을 rotate하면 새 refresh token과 access token을 함께 commit한다. Provider가
refresh token을 생략하면 그 provider 계약이 omission을 명시적으로 허용하는 경우에만
현재 refresh token을 유지한다. Concurrent response에서 패한 generation은 더 새
generation을 overwrite할 수 없다.

| Case | 필수 결과 |
|---|---|
| fresh access token과 complete scope | fresh capability decision 이후에만 사용 |
| expiring/expired access token과 valid refresh | 작업 전 single-flight refresh. 성공 전 cursor advance 금지 |
| 한 account의 두 refresh | network exchange 하나. 다른 caller는 committed generation 또는 typed failure 관측 |
| 다른 account의 refresh | lock, token, health, rate limit, result 격리 |
| rotated refresh token | atomic generation swap. old token 사용 불가 및 삭제 |
| `invalid_grant`, provider revoke 또는 one bounded retry 뒤 authenticated `401` | account work 취소, provider 계약에 따라 authorization stale/revoked, unusable access material 삭제, 명시적 reauthorization 필요 |
| refresh token 없는 expired token | `stale` + `reauthorization_required`. silent browser flow 금지 |
| refresh 후 partial/missing scope | 영향받는 capability deny, 관련 없는 narrower capability 유지, explicit upgrade 필요 |
| account switch | old account work 취소/quarantine, UI selection만 변경. token/cursor/cache/decision 재사용 금지 |

Transient network/5xx/429 failure는 검증하고 cap한 provider `Retry-After`와 bounded
exponential backoff를 사용한다. Global disconnect가 아니라 account/install health와
actionable remediation을 만든다. Terminal auth failure를 transient error로 retry하지
않는다.

### 10. Revoke, policy change, account loss는 즉시 효력이 생긴다

Capability broker는 모든 page, retry, projection read, source-open, suggestion input,
external action 전에 다시 평가한다. Authority change가 이미 발급한 decision을
invalidate하도록 cancellation epoch도 publish한다.

Product-consent withdrawal, install-grant revoke, OAuth revoke/401, account access loss,
managed-policy narrowing 또는 Keychain authority loss 시:

1. account/install cancellation epoch를 증가시키고 새 작업을 reject한다.
2. 다음 cancellation-safe await에서 in-flight acquisition/action을 취소한다.
3. 영향받는 page/action의 cursor advance와 projection publication을 atomic하게
   거부한다.
4. 사용할 수 없는 token handle과 cached authorization/readiness decision을 지운다.
5. Source data에 ADR-030 `access_revoked` 및 content/tombstone 동작을 적용한다.
6. authority와 영향받는 capability를 담은 metadata-only receipt를 emit한다.
7. authorization을 자동 시작하지 않고 typed remediation을 노출한다.

이미 irreversible provider side effect를 만든 operation은 `outcome_unknown`을
기록하고 reconciliation이 필요하다. Revoke가 rollback을 주장할 수 없다. 이후 retry는
old decision 또는 token generation을 사용하지 않는다.

### 11. Disconnect와 uninstall의 cleanup ownership을 명시한다

Account disconnect는 정확히 하나의 `ExtensionAccountKey`에 영향을 준다:

1. 해당 account의 작업을 disable하고 취소한다.
2. 지원되는 경우 pinned endpoint로 provider revoke를 시도한다.
3. Remote revoke가 실패해도 local Keychain credential을 삭제한다.
4. ADR-030에 따라 account cursor, retry material, raw payload, projection, cache를
   지운다.
5. 보존한 minimized provenance와 suppression tombstone에 올바른 lifecycle을
   표시한다.
6. Package, installation, 관련 없는 account, 사용자가 확인한 ADR-028 to-do를
   유지한다.

Remote revoke failure는 visible metadata-only cleanup receipt다. 제품은 revoke retry만을
위해 credential을 보존하지 않는다. Reconnection은 새 access epoch를 만들고 explicit
authorization을 수행한다.

Uninstall은 모든 account에서 이 순서를 반복한 다음, ADR-029가 요구하는 대로 durable
cleanup receipt가 존재한 뒤에만 installation grant를 revoke하고 package/runtime
metadata를 제거한다. Local credential 또는 content cleanup이 미완료이면 UI는
disconnected/uninstalled를 보고하지 않는다.

### 12. Capability broker와 audit API는 metadata만 노출한다

`maekon-core`는 다음 operation을 가진 additive object-safe
`ExtensionAuthorizationBrokerPort`를 정의한다:

- fresh authority snapshot에 대해 하나의 account/capability/operation/resource
  request 평가
- explicit authorization 또는 scope-upgrade flow 시작/취소
- account authorization/readiness snapshot 조회
- revoke, policy, account, Keychain change 통지
- 이미 authorize된 network adapter용 opaque short-lived credential lease 획득
- durable cleanup receipt를 가진 account disconnect

Lease는 non-serializable, process-local, account/operation/audience-bound,
generation-bound, cancellable하다. Public application DTO는 token이나 reusable
credential handle을 절대 전달하지 않는다.

최소 audit field는 event ID, timestamp, extension/install/opaque account ref, bounded
action/capability, decision/reason, requested/granted scope fingerprint,
manifest/grant/policy/credential generation, provider ID, outcome, retryability,
correlation ID다. Audit는 secret, raw scope consent page, provider body, URL/query
string, tenant name, email, remote content를 제외한다.

Rate limit과 auth health는 install/account 및 provider operation별로 독립 추적한다.
UI remediation은 provider body text 없이 `reauthorize`, `grant_scope`, `retry_after`,
`check_keychain`, `policy_denied`, `account_unavailable`을 표시할 수 있다.

### 13. UI readiness는 backend truth보다 앞설 수 없다

Backend snapshot은 모든 authoritative axis, effective capability decision, missing
scope/capability delta, credential expiry bucket, freshness, health, rate-limit state,
remediation key를 포함한다. Frontend는 locally 더 넓은 state를 파생하지 않는다.

OAuth callback만으로 account가 `ready`가 되지 않는다. Readiness에는 성공한
state/PKCE validation, token storage, account-subject verification, scope comparison,
current product consent, install grant, managed policy, Keychain availability, provider
readiness, fresh broker decision이 필요하다.

Optimistic UI는 `authorizing` 또는 `checking`만 보여줄 수 있고 `ready`나 `syncing`을
보여주지 않는다. Unknown backend, auth, scope, account, policy state는 unavailable이다.
UI copy는 i18n-driven이며 account-safe label과 opaque identifier를 분리해 표시한다.

## 동결 불변식 (Frozen Invariants)

다음 항목을 변경하려면 새 ADR 또는 명시적 update가 필요하다:

1. OAuth scope와 Maekon product consent는 독립적이며 use time에 교집합을 구한다.
2. Extension/install/account identity는 모든 credential, cursor, grant, refresh, cache,
   health, rate limit, operation을 scope한다.
3. Installed, enabled, authorized, granted, ready, syncing은 하나의 authoritative
   boolean이 아니다.
4. Managed policy는 authority를 좁힐 수 있지만 user authority를 만들거나 넓힐 수
   없다.
5. Secret value는 OS Keychain과 process-local bounded lease에만 존재한다.
6. Revoke는 영향받은 cursor를 advance하거나 새 projection을 publish하지 않고 새
   작업과 in-flight 작업을 invalidate한다.
7. Provider body, redirect target, raw error description은 log, audit, telemetry,
   public DTO 또는 UI state에 들어가지 않는다.
8. UI readiness는 fresh backend broker decision보다 앞서지 않는다.

## 결과 (Consequences)

### 긍정적

- 여러 account와 installation이 token, cursor, refresh lock, capability decision 또는
  projected context를 실수로 공유할 수 없다.
- Provider가 넓은 OAuth scope를 grant해도 product consent가 authoritative하게
  유지된다.
- Revoke와 account switching에 deterministic cancellation과 cleanup이 있다.
- UI가 credential을 노출하지 않고 partial scope와 remediation을 설명할 수 있다.

### 부정적

- 현재 provider-only `OAuthPort`를 Extension에 그대로 재사용할 수 없다.
- Account-scoped cancellation epoch, token generation, atomic cleanup에 추가 storage와
  property test 작업이 필요하다.
- Stable subject identity, pinned endpoint, bounded error response 또는 explicit revoke
  behavior가 없는 provider는 unavailable로 남는다.

### 중립적

- 기존 standalone AI-provider OAuth는 현재 ownership 경계에 그대로 남는다.
- Provider-specific endpoint, scope map, account discovery는 이 계약을 따라야 하는
  후속 구현으로 남는다.

## 검토한 대안 (Alternatives Considered)

**A. OAuth scope를 Maekon consent로 취급.** Provider grant는 screen/OCR/full-text
collection 또는 package capability를 authorize할 수 없으므로 기각했다.

**B. `provider_id`를 account key로 재사용.** 같은 provider의 account와 installation이
credential, refresh lock, cursor를 공유하게 되므로 기각했다.

**C. 하나의 `connected` boolean 저장.** Install, enablement, grant, scope, policy,
health, operation failure를 숨기므로 기각했다.

**D. Refresh를 globally retry하고 마지막 response 수용.** Cross-account blocking과
refresh-token rotation race가 valid credential을 overwrite할 수 있으므로 기각했다.

**E. Provider redirect를 따른 뒤 sanitize.** 307/308이 sanitize 전에 credential form
body를 replay할 수 있고 DNS rebinding이 destination을 바꿀 수 있으므로 기각했다.

**F. Remote revoke retry를 위해 disconnect 뒤 token 보존.** Provider가 unavailable해도
local revoke는 완료해야 하므로 기각했다.

## 검토 및 구현 게이트 (Review and Implementation Gates)

본 ADR이 `Accepted`가 되기 전에 reviewer는 identity derivation, scope mapping,
Keychain namespace, lifecycle precedence, endpoint policy, cancellation epoch, refresh
rotation, cleanup ordering, metadata-only audit를 승인해야 한다. 이후 구현 test는 다음을
포함해야 한다:

1. OAuth scope/product-consent intersection과 managed-policy deny precedence
2. 같은 provider의 두 account 및 두 installation
3. enable vs authorize vs grant vs ready state derivation
4. duplicate callback, wrong/expired state, wrong account, PKCE downgrade, callback
   cancellation
5. redirect 301/302/307/308, disallowed host/path/port, private/metadata IP,
   IPv4-mapped IPv6, DNS-rebinding 시도
6. concurrent same-account refresh, different-account refresh, rotated refresh token,
   losing generation, expired access token, missing refresh token
7. partial/extra/missing scope와 declined incremental upgrade
8. active page 중 account switch, page 중 consent revoke, OAuth 401, policy narrowing,
   Keychain loss
9. remote revoke 성공/실패 disconnect/uninstall, cursor/projection cleanup, confirmed
   to-do retention
10. provider error-body, redirect, token, code, state, verifier, email, raw ACL이
    log/audit/telemetry/public DTO에 없음을 검증
11. per-account rate-limit 및 auth-health 격리
12. backend-not-ready/unknown state UI non-regression과 en/ko parity

## 알려진 후속 작업 (Known Follow-ups)

1. **#8587 source runtime** — Accepted broker decision과 cancellation-epoch binding
   이후에만 page schedule
2. **#8589 encrypted ledger** — credential 없이 account-scoped cursor와 cleanup receipt
   persist
3. **Provider connector ADR** — provider별 signed endpoint metadata, minimum scope map,
   stable subject derivation, revoke semantics, evidence 정의
4. **OAuth adapter 구현** — 현재 provider-only port의 wire 계약을 바꾸지 않고
   account-scoped port 추가

## 개정 2026-07-21: 리뷰 해소

3-loop 적대적 리뷰(보안 devils-advocate + rust-core) 후 Proposed→Accepted. 정본은 영문
[Amendment 2026-07-21](./ADR-031-extension-authorization-account-boundary.md#amendment-2026-07-21-review-resolutions).
핵심 해소(계약):
- **BLOCKING(§1)**: `local-account-key`=설치당 1회 생성하는 Keychain 전용 시크릿. SQLite/config/
  텔레메트리/네트워크에 미기록, `device_identity.device_id`와 독립. 설치별로 다른 `account_id`
  생성(경계 불평등 보장). 분실/회전 시 해당 계정 재인증(사용자 가시 이벤트).
- §12 `ExtensionAuthorizationBrokerPort`=`#[async_trait]`; §3 "global Maekon consent" AND항은
  신규 `ConsentPermissions` 필드 필요(그 전엔 fail-closed), P02(#8587)가 도입; §10 cancellation
  epoch은 broker 소유(ADR-030 `access_epoch_id`와 별개); §7 DNS-rebinding은 커넥터 교체 필요 가능.
- 어휘=§5 canonical, `task_source_refs.account_subject_ref`(v49) 재사용, 다음 마이그레이션 v50.

## 관련 문서 (Related Docs)

- `docs/architecture/ADR-008-network-resilience-patterns.md`
- `docs/architecture/ADR-021-config-consent-core-placement.md`
- `docs/architecture/ADR-026-async-storage-convergence-consent-port.md`
- `docs/architecture/ADR-029-extension-package-runtime-boundary.md`
- `docs/architecture/ADR-030-work-context-envelope-convergence.md`
- `crates/maekon-core/src/ports/oauth.rs`
- `crates/maekon-network/src/oauth/mod.rs`
- `crates/maekon-network/src/oauth/refresh_coordinator.rs`
- `crates/maekon-network/src/oauth/token_exchange.rs`
- `crates/maekon-storage/src/keychain.rs`
