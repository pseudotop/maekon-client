[English](./ADR-025-codex-chatgpt-subscription-tos-gate.md) | [한국어](./ADR-025-codex-chatgpt-subscription-tos-gate.ko.md)

# ADR-025: OpenAI Codex / ChatGPT 구독 ToS 게이트 (App-Server 경로)

**상태**: Accepted
**날짜**: 2026-06-04
**범위**: `specs/providers/provider-surface-catalog.json`, `src-tauri/src/session_manager/factory.rs`, `src-tauri/src/provider_adapters/llm_resolver.rs`
**관련**: ADR-024 (대화 콘텐츠 가드 포트), ADR-021 (config/consent core 배치), ADR-019 (에러 코드)
**관련 이슈**: #4861 (Epic E21), #4863 (PoC), #4866 (세션 resume), #4868 (`chatgptAuthTokens` 주입), #4871 (feature-flag 롤아웃 + degrade), #4872 (mock 하니스 + R4 귀속)

> **이 문서는 엔지니어링 관점의 이용약관(ToS) *리스크* 분석이며, 법률 자문이나 법무 승인이 아니다.** 공개된 OpenAI/Anthropic 정책·개발자 문서가 무엇을 말하는지 정리하고, 이를 maekon 아키텍처에 매핑하여, 어떤 E21 작업을 어떤 조건으로 진행할 수 있는지 결정한다. **NEEDS_LEGAL_SIGNOFF**로 표시된 항목은 출하 전 사람/법무 판단이 필요하다. §5 참조.

---

## 배경 (Context)

E21은 Codex 클라이언트 통합을 매-턴 `codex exec`에서 영속 `codex app-server` JSON-RPC 세션으로 전환한다(#4861). 이슈 #4884(리뷰 발견 B4/R5)는 PoC 착수(#4863)를 *차단*하는 거버넌스 게이트를 제기했다: maekon의 `provider-surface-catalog.json`은 Codex surface를 `credential_kind: cli_subscription`으로 선언한다 — maekon이 API 키가 아니라 **사용자가 설치한 Codex CLI의 ChatGPT 구독**을 차용한다. 여기에 계획된 `chatgptAuthTokens` 호스트 토큰 주입(#4868)이 더해지면 **비대화형 자율 에이전트가 인간의 ChatGPT 구독 quota를 적극 소비**하게 된다. #4884 이전에는 OpenAI ToS / 계정 귀속 / quota / 책임 경계가 11개 E21 이슈 중 **0건**, catalog 필드 **0건**으로 문서화되어 있었다.

maekon은 대화형 코딩 어시스턴트와 세 가지 면에서 본질적으로 달라 게이트를 더 날카롭게 만든다:

1. **자율·비대화형 턴** — 사람 개입 없이 턴을 구동할 수 있어, "프로그램적 추출처럼 보이는 자동화" 그레이존에 정확히 해당.
2. **화면 캡처/감시 제품** — 턴이 캡처된 화면 콘텐츠를 운반할 수 있고, 이는 *제3자*의 사적/민감 정보를 포함할 수 있음.
3. **배포형·유료 가능 제품** — 단일 개발자가 자기 머신에서 자기 CLI를 쓰는 것이 아님.

본 ADR은 sub-feature별 결정 매트릭스로 게이트를 해소한다.

### 증거 신뢰 등급 (Confidence tiers)

게이트 *해제*는 HIGH 신뢰 증거에만 의존한다. MEDIUM 신뢰 증거는 *제한*(보수적=안전 방향)에만 사용한다.

- **HIGH (직접 fetch/read)**: `developers.openai.com/codex/auth` 및 `/app-server`; in-repo catalog 및 `factory.rs` 사실(worktree HEAD `533b9c27d`에서 독립 리뷰 에이전트 3개 + 작성자가 재검증).
- **MEDIUM (WebSearch 종합; `openai.com/policies/*`와 `help.openai.com`이 자동 fetch에 HTTP 403, `web.archive.org`는 도구 차단)**: `row-terms-of-use`, 계정 공유 정책, 소비자-vs-API 데이터 학습 기본값의 정확한 문구. 프로덕션 승인 전 인증된/비차단 환경에서 정확한 문구 재확인 필수(§5, 잔여 항목 1).

## 결정 (Decision)

### 결정 매트릭스

| # | 대상 | 판정 | 근거 |
|---|------|------|------|
| 1 | **#4863 app-server PoC** — ChatGPT-managed OAuth (CLI가 인증 소유) | **CONDITIONAL — 사람-개시(attended) 턴 한정 클리어** | HIGH |
| 2 | **app-server vs 기출하 `exec`** 전송 (전송 축만) | **ALLOWED** (동일 ToS 토대) | HIGH |
| 3 | **비대화형/자율 quota 소비** (소비자 구독 기준) | **CONDITIONAL — 무인 턴은 API-key 강제** | HIGH+MEDIUM |
| 4 | **#4868 `chatgptAuthTokens` 호스트 토큰 주입** (OpenAI "External tokens", experimental) | **제품 기본값 BLOCKED — NEEDS_LEGAL_SIGNOFF** | HIGH+선례 |
| 5 | **상업/유료 배포** wrapper | **NEEDS_LEGAL_SIGNOFF** | 미해결 질문 |
| 6 | **데이터 거버넌스**: 소비자 구독 경유 캡처 화면 콘텐츠 | **CONDITIONAL — 캡처 콘텐츠는 API-key 기본** | HIGH+MEDIUM |

### 1. #4863 app-server PoC — 해제, 단 사람-개시 턴 한정

app-server PoC 및 #4871/#4872 flag-rollout + degrade + handshake 작업은 기출하 `exec` 경로 대비 **신규 ToS 자격증명 surface를 추가하지 않으므로** experimental·flag-gated 작업으로 **진행 클리어**된다:

- `provider_surface.openai.codex_app_server`는 기출하 `provider_surface.openai.subprocess_cli`(exec)와 **동일한** `credential_kind: cli_subscription` 및 `auth_probe_mode: codex_login_status_text`를 사용한다. 동일 공식 바이너리를 통해 **사용자 자신의 `codex login`**을 구동한다 — OpenAI의 "ChatGPT managed" 모드로, 토큰 영속/갱신을 *Codex*가 소유하며 maekon이 아니다.
- `chatgptAuthTokens`는 **코드 참조 0건**(grep 확인) — 호스트 토큰 주입은 오늘 존재하지 않는다.
- `clientInfo.name = "maekon"` 귀속이 배선되어 있고(`factory.rs:316-320`) 전체 outbound JSON-RPC wire 스냅샷으로 계약 테스트된다(`crates/maekon-network/tests/codex_app_server_integration.rs::initialize_request_contract_snapshot`).

**조건 (전부 필수):**
1. **CLI가 OAuth 흐름을 end-to-end 소유** — `auth_probe_mode: codex_login_status_text` 유지; maekon은 하부 ChatGPT 토큰을 읽기·영속·중계·주입 금지(이는 결정 4).
2. **인간 1인 단일 사용자 바인딩만** — 한 인간 자신의 구독; 자격증명 공유·멀티유저 프록시·멀티테넌트 중계 금지.
3. `stability: experimental` + `preferred_for_product_auth: false` 유지.
4. `clientInfo.name` 귀속 배선·계약 테스트 유지.
5. PoC 이상으로 승격 전 catalog `tos_notes` + `usage_attribution` 백필(결정 7).
6. **클리어는 좁다.** *전송*과 *사람-개시* 사용만 포함하며, 자율 소비(결정 3)나 캡처-콘텐츠 라우팅(결정 6)을 축복하지 않는다.

**근거**: "이미 출하됨" ≠ "ToS 판정됨". exec 경로는 명시적 ToS 리뷰 없이 출하되었으므로, app-server는 exec의 자율-소비·데이터-거버넌스 노출을 *상속*할 뿐 해소하지 않는다. PoC 클리어는 출하 코드와의 전송 동등성 + OpenAI 자신의 app-server 추천("a deep integration inside your own product: authentication, conversation history, approvals, and streamed agent events", `developers.openai.com/codex/app-server`, HIGH)에 근거하며, maekon이 구독을 어떻게 소비하는지에 대한 포괄적 축복이 아니다.

### 2. app-server vs exec 전송 — 허용, 동일 토대

전송 메커니즘(공식 `codex` 바이너리를 `app-server` JSON-RPC로 구동 vs `exec` 일회성 호출)은 **ToS 관련 축이 아니다** — 둘 다 동일 바이너리의 자체 로그인을 통해 사용자 자신의 구독을 소비한다. 한쪽에 적용되는 ToS 제약은 다른 쪽에도 동일 적용되며 공유 `tos_notes`에 **한 번** 인코딩해야 한다. `exec` surface는 이미 출하됨(`preferred_for_product_auth: true`); app-server를 experimental·flag-gated·fallback-보호 sibling으로 추가하는 것은 신규 자격증명 surface를 더하지 않는다.

### 3. 비대화형/자율 소비 — 무인 턴은 API-key 강제

이것이 **load-bearing 잔여 리스크**이자 #4884의 문자 그대로의 주제("codex app-server를 구동해 인간 사용자의 ChatGPT 구독을 **비대화형으로** 소비")다. OpenAI 자신의 문서가 프로그램적/자동화 사용을 반복적으로 **API key**로 유도한다:

- *"We recommend API key authentication for programmatic Codex CLI workflows, such as CI/CD jobs."* (`developers.openai.com/codex/auth`, HIGH)
- *"Don't expose Codex execution in untrusted or public environments."* (동일, HIGH)
- *"Access tokens are intended for trusted scripts, schedulers, and private CI runners."* (동일, HIGH)

**조건:**
1. **무인/스케줄 턴은 API-key(Platform 과금) surface 선호/강제.** 소비자 구독 인증은 **사람-개시/승인-게이트** 턴에만 한정.
2. 소비자 구독에서 자율 턴은 **대화형처럼 페이싱** — 재시도 폭주·버스팅 금지, "프로그램적 추출이나 rate-limit 우회처럼 보이는" 행위 금지(소비자 ToU "what you cannot do", MEDIUM).
3. `cli_subscription` surface에 `preferred_for_product_auth: false` 유지.
4. `is_external() = true` 프라이버시 가드(ADR-024) + R6 read-only sandbox clamp는 **필요하지만 충분하지 않음** — egress를 게이트할 뿐 소비/귀속 우려를 해소하지 않음.

### 4. #4868 `chatgptAuthTokens` 호스트 토큰 주입 — 제품 기본값 BLOCKED

OpenAI "External tokens" 모드는 *"experimental and intended for host apps that already own the user's ChatGPT auth"*(`developers.openai.com/codex/app-server`, HIGH)다. 호스트가 공급/영속한 토큰으로 구독을 구동하는 것은 **2026-01 Anthropic이 명시적으로 차단한 형태와 구조적으로 동일**하다 — Anthropic은 OAuth 흐름을 가로채 access token을 추출해 네이티브 클라이언트 *밖에서* 호출한 도구(OpenClaw, OpenCode, Roo Code, Goose)에 대해 Consumer ToS를 집행했다. Anthropic이 *허용*으로 남긴 패턴은 공식 CLI 바이너리를 subprocess로 호출하는 것(결정 1/2 패턴)이었다. 주요 랩이 호스트-토큰 모드를 정확히 금지할 수 있고 *실제로 했다*.

**판정: 제품 기본값 BLOCKED; 출하 전 NEEDS_LEGAL_SIGNOFF.** "OpenAI가 experimental로 제공한다"는 낙관적 해석으로 출하 금지. 추후 추진 시, **모두** 충족할 때만 허용: (a) 최종 사용자가 자기 토큰을 직접 공급; (b) maekon이 소유 세션 범위를 넘어 사용자/세션 간 저장·중계·공유하지 않음; (c) OpenAI의 명시적 서면 확인 획득. #4863/#4871/#4872와 **별도 게이트**로 취급 — 유일한 진짜 신규 ToS surface다.

### 5. 상업/유료 배포 — NEEDS_LEGAL_SIGNOFF

OpenAI는 `openai/codex` Discussion #8338에서 "유료 wrapper 제작+판매, 사용자가 자기 구독 지참" 질문에 **명시적으로 답하지 않았다**(OpenAI 메인테이너가 법무로 미룸). OpenClaw/Altman 커뮤니티 지지는 **무료 OSS** 도구 대상이라 **유료·배포형·자율 감시** 제품의 약한 선례다. Apache-2.0 라이선스는 *코드* 재사용을 허용할 뿐 *구독 소비 약관*에 대해선 아무것도 말하지 않는다.

**방어 가능한 구성만**: 사용자가 공식 `codex` 바이너리를 subprocess로 통해 자기 구독 지참, 사용자가 `codex login` 소유(`auth_probe_mode: codex_login_status_text`), 호스트 토큰 주입 없음, 멀티유저 프록시 없음. OpenClaw/Altman 유추나 Discussion #8338을 green light로 의존 **금지**. "Account 접근 재판매/리스" 또는 사용자 구독으로 "third-party service를 구동"으로 읽힐 framing/아키텍처 회피(소비자 ToU, MEDIUM).

### 6. 데이터 거버넌스 — 캡처 콘텐츠는 API-key 기본

화면 캡처 제품으로서 maekon에 특유한 점: **소비자 ChatGPT 플랜은 대화를 기본 학습(opt-out)**, API/business 경로는 **기본 no-train(opt-in)**(MEDIUM — `help.openai.com` 데이터-사용 문서 + `openai.com/enterprise-privacy`, 403 차단, 승인 전 재확인). 캡처된 화면 콘텐츠(제3자의 사적/민감 정보 포함 가능)를 소비자 구독 인증으로 라우팅하면 해당 콘텐츠가 **기본적으로 OpenAI 모델 학습에 사용될 수 있다**.

**조건:**
1. **캡처 화면 콘텐츠**를 운반하는 모든 흐름은 **API-key(Platform, 기본 no-train) 경로가 거버넌스 정답 기본값**; 소비자 구독 인증을 캡처 콘텐츠의 기본으로 두지 않음.
2. 캡처 콘텐츠에 소비자 구독을 쓰는 경우, 콘텐츠가 기본적으로 학습에 사용될 수 있고 제3자 데이터를 함의할 수 있다는 명시적 런타임/`tos_notes` 경고 노출.
3. `is_external()` 가드 + R6 clamp는 egress를 게이트할 뿐 **학습-데이터 사용은 아님** — 여기선 필요하나 충분치 않음.

### 7. Catalog 변경 (`provider-surface-catalog.json`)

위 내용을 **양쪽** OpenAI `cli_subscription` surface(`provider_surface.openai.subprocess_cli` + `provider_surface.openai.codex_app_server`)에 머신-판독 필드로 인코딩:

- `usage_attribution`: `{ mechanism: "client_info_name", value: "maekon", target: "OpenAI Compliance Logs Platform", evidence: "JSON-RPC initialize clientInfo.name" }`.
- `tos_notes`: 배열로 — (a) `cli_subscription`은 사용자 자신의 `codex login` 구동(ChatGPT-managed, 호스트 토큰 미주입); (b) 구독 인증 = OpenAI 문서상 대화형/신뢰-사설 전용, 무인/스케줄 소비는 비선호 그레이존 → API-key 선호; (c) 429 시 back off + 사용자 고지, 소비 지속 위해 re-route 금지(rate-limit 우회 금지); (d) 소비자 플랜 train-by-default → 캡처 화면 콘텐츠는 API-key 선호; (e) `host_injects_token: false`.
- `host_injects_token: false` 머신-체크 가능 boolean 추가, **CI lint로 강제**(선언만이 아니라) — 미래 #4868-게이트 surface 밖에서 maekon이 `true`로 설정하지 않음을 단언.
- 양쪽 surface `references`에 기존 `developers.openai.com/codex/auth/`와 함께 `https://openai.com/policies/row-terms-of-use/` 추가.
- #4868 착수 시, 호스트-주입 토큰용 **별도** surface(또는 `credential_kind` 값) 추가 — 자체 `tos_notes`로 최고위험 + Anthropic 선례 표기, `preferred_for_product_auth: false`, `stability: experimental`.

### 8. 429 / rate-limit 정책 (#4871)

**검증된 갭**: `factory.rs:288-296`은 blanket `Err(err) => … build_codex_exec_session(…, "app_server_failed")` catch-all이며, `codex_app_server.rs` / `codex_app_server_session.rs` 어디에도 429/`RateLimit`/quota 분류가 **없다**(grep 확인). 429를 `codex exec`로 degrade하면 **같은** `cli_subscription` quota 벽을 다시 칠 뿐이고, rate limit을 우회해 소비를 지속하는 것은 소비자 ToU "what you cannot do"가 금지하는 바("circumventing rate limits", MEDIUM)다.

**정책**: app-server(또는 exec) 경로의 429 / rate-limit / quota 소진 시, **back off + 사용자에게 한도 고지** — silent retry 금지, 소비 지속을 위한 대체 auth/transport re-route 금지. 이를 위해 어댑터에서 429를 전송 실패와 구별 분류해 `factory.rs` fallback이 generic degrade-to-exec로 삼키지 않게 해야 한다. 후속으로 추적(§알려진 후속 1).

## 결과 (Consequences)

### 긍정

- PoC(#4863)와 이미 머지된 #4866/#4871/#4872 작업이 명시·문서화된 조건 하에 unblock — E21 진행 가능.
- 진짜 신규/불확실 리스크 2개(#4868 호스트-토큰 주입, 유료 배포)가 낙관적 해석으로 출하되지 않고 명시적 법무 게이트 뒤로 격리.
- 감시 제품에 대해 load-bearing이 되기 전 데이터-거버넌스 기본값(캡처 콘텐츠는 API-key) 확립.
- catalog가 머신-판독 `tos_notes`/`usage_attribution`/`host_injects_token`을 얻어 정책이 산문이 아니라 강제 가능해짐.

### 부정

- 광범위 롤아웃 전 후속 2개 필요: 429 back-off 분류 + `host_injects_token` CI lint.
- 일부 유용한 흐름(소비자 구독의 무인 자율 턴)이 의도적으로 API-key로 유도되어, 정액 구독 대신 사용량 과금 비용 발생.
- MEDIUM 신뢰 ToU 인용은 *제한적* 결론이 종합 문구에 의존함을 뜻하며 재확인 필요; *허용적* 클리어는 이에 의존하지 않음.

### 중립

- exec 경로는 `preferred_for_product_auth: true` 유지; app-server는 `CodexAppServerRollout`(Off 기본) 뒤 experimental/flag-gated 유지.

## 고려한 대안 (Alternatives Considered)

**A. 전체 `cli_subscription` Codex 통합을 완전 법무 승인까지 BLOCKED 처리.** 기각: 전송/PoC는 출하 exec 코드 대비 신규 자격증명 surface를 더하지 않고, OpenAI가 제품 통합용으로 app-server를 명시 추천한다; PoC를 막으면 새롭지도 않고 PoC가 해소할 것도 아닌 리스크로 E21을 정체시킨다.

**B. OpenClaw/Altman 선례로 `cli_subscription`을 완전 ALLOWED 처리.** 기각: 그 선례는 커뮤니티 출처·무료 OSS 도구 대상이며 OpenAI는 유료-wrapper나 자율-소비 질문에 답한 적 없다. green light로 읽으면 미판정 리스크를 해소된 것으로 세탁한다.

**C. `chatgptAuthTokens` 호스트-토큰 주입을 지금 출하(제공되는 experimental 모드이므로).** 기각: 2026-01 Anthropic이 집행 차단한 패턴과 구조적으로 동일; "experimental" 라벨은 배포형 제품의 출하 허가가 아니다.

## 알려진 후속 (Known Follow-ups)

1. **429 back-off 분류 (#4871 후속)** — `codex_app_server.rs`/`session.rs`에서 429/rate-limit/quota를 구별 분류해 `factory.rs`가 quota 벽을 같은-quota exec로 degrade하지 않게; back off + 사용자 고지. 소/중 규모.
2. **`host_injects_token` CI lint** — `cli_subscription` surface에서 `host_injects_token: false`를 머신 강제. 소 규모.
3. **인증된 ToU 재fetch** — `openai.com/policies/row-terms-of-use`, 계정 공유, 소비자-vs-API 데이터 학습 기본값의 정확한 문구를 비차단 환경에서 확정; MEDIUM 인용을 HIGH로 승격. 제한적 결론의 프로덕션 승인 전 필수.
4. **캡처 콘텐츠/무인 턴의 API-key surface 선호** — resolver를 배선해 캡처-콘텐츠·무인 흐름이 `cli_subscription`보다 API-key surface를 선호.
5. 결정 4(#4868)와 5(유료 배포)의 출하 전 **법무 승인**.

## 관련 문서 (Related Docs)

- `docs/architecture/ADR-024-conversation-content-guard-port.md` — `is_external()` chat-egress 가드(위에서 참조한 necessary-not-sufficient 통제)
- `docs/architecture/ADR-021-config-consent-core-placement.md` — consent 경계
- `specs/providers/provider-surface-catalog.json` — 본 ADR이 다스리는 surface
- `developers.openai.com/codex/auth`, `developers.openai.com/codex/app-server` — HIGH 신뢰 인증-모드 + 추천 출처
- `openai/codex` Discussion #8338 — 미답변 유료-wrapper 질문

---

## Update 2026-06-04 — Decision 4 심층 해소 (#5034): 호스트-토큰 주입 = WON'T-IMPLEMENT

원 Decision 4는 `chatgptAuthTokens` 호스트-토큰 주입을 **BLOCKED / NEEDS_LEGAL_SIGNOFF**로 남겼다. 이슈 #5034가 전용 엔지니어링 ToS-리스크 리서치로 결론을 도출했다. **이는 엔지니어링 ToS-리스크 분석이며 구속력 있는 법무 자문이 아니다.**

### 결정: maekon은 `chatgptAuthTokens` 호스트-토큰 주입을 구현하지 않는다.

#5034를 문서화된 **wontfix**로 해소. 두 `cli_subscription` surface(`provider-surface-catalog.json` L854 + L1041)의 `host_injects_token: false` + CI-lint follow-up 유지. 3개 독립 리서치 렌즈가 수렴하고 adversarial 리뷰를 생존.

### 근거

1. **기능적 공백 없음(in-repo 검증).** host-injection만이 serve하는 maekon 시나리오 부재. tree의 유일 "headless" 경로는 GUI-less 빌드의 데스크탑-알림 UI 억제 fallback(`agent_runtime_support.rs` `LogOnlyNotifier`)일 뿐, 사용자가 interactive `codex login`을 못 하는 배포가 아님. 대화형 구독 사용은 **ChatGPT-managed OAuth**(사용자 자기 `codex login`, Codex가 flow+refresh 소유, 토큰이 사용자 머신을 떠나지 않음, maekon 미보관)로, 무인/자동+캡처-콘텐츠는 **API-key(Platform)** 경로(Decision 3/6)로 충족. host-injection은 "maekon이 사용자 ChatGPT 토큰 custody+refresh"라는 리스크만 추가하고 오늘 없는 기능은 0.

2. **최고 ToS 리스크 — 주요 랩이 집행한 패턴과 정확히 일치.** OpenAI는 `chatgptAuthTokens`를 experimental로만 문서화: *"experimental and intended for host apps that already own the user's ChatGPT auth lifecycle"*(`developers.openai.com/codex/app-server`, HIGH; `capabilities.experimentalApi=true` 게이트 = *불안정* 기능 opt-in, 출하 승인 아님). auth 문서는 end-user에게 managed-OAuth+API-key만 기술하고 토큰을 *"password처럼 다루고 공유 말 것"* 경고(`developers.openai.com/codex/auth`, HIGH). **OpenAI는 distributed third-party 제품이 사용자 개인 ChatGPT 구독 토큰을 custody/주입하는 것을 affirmatively 허용한 적 없음** — "auth lifecycle 소유" 단서는 1st-party/enterprise 정체성 컨텍스트로 읽힘. 한편 **Anthropic**(2026-01-09 집행, 2026-02-20 clarification, HIGH)은 구조 동일 패턴을 명시 위반화: *"Using OAuth tokens obtained through Claude Free, Pro, or Max accounts in any other product, tool, or service — including the Agent SDK — is not permitted and constitutes a violation of the Consumer Terms of Service"*, server-side 집행(origin/fingerprint/user-agent/behavioral, secondary 보도 — The Register), 동기 = OpenAI 구독에도 동일 적용되는 구독-vs-API **토큰 차익거래**.

3. **상쇄 이득 없음.** (1)이 대안이 모든 need를 이미 충족함을 보이므로, host-injection은 순수 downside: 신규 기능 0에 토큰-custody + Anthropic-선례 집행 리스크를 떠안음. (참고: OpenAI의 OpenCode/Cline 등 third-party 도구 용인은 program-gated OSS-maintainer 형태로 보도됨 — secondary가 GitHub-stars 임계 언급, 미검증 secondary로 취급 — distributed 제품의 host-inject 포괄 허가가 아님.)

### 향후 재검토 전제(전부 충족 필요; 오늘 하나도 미충족 → 본 해소가 아닌 신규 ADR)

(i) interactive `codex login` 불가 + API-key 경로로도 턴 처리 불가인 구체적 기능 공백(오늘 반증됨); (ii) 최종 사용자가 자기 토큰 직접 공급; (iii) 단일-세션 custody, relay/공유 없음; (iv) distributed third-party 제품이 사용자 개인 ChatGPT 구독 토큰을 프로덕션 host-inject 가능하다는 **OpenAI 명시적 서면 확인**(experimental 문서·침묵 아님); (v) 인증된 ToU re-fetch(Follow-up #3) + 정식 법무 sign-off(Follow-up #5); (vi) 구현 시 별도 surface/`credential_kind` + `host_injects_token: true`/`stability: experimental`/`preferred_for_product_auth: false` + Anthropic-선례 `tos_notes` — 기존 `cli_subscription` surface의 invariant를 절대 뒤집지 않음.

### 신뢰도 + 잔여

HIGH: `developers.openai.com/codex/{app-server,auth}` 인용(직접 fetch) + in-repo 기능-공백-없음. MEDIUM: `openai.com/policies/row-terms-of-use` 계정-공유 조항(403 차단 → secondary; Follow-up #3 재확인) + OpenCode program-gating 임계(secondary). Anthropic clarification 인용은 HIGH(primary 보도). 잔여 법무 항목(Follow-up #3 인증 ToU re-fetch; Follow-up #5 Decision 4+5 법무 sign-off)은 재검토 개시 시 formally open이나, 엔지니어링 결론은 독립적으로 성립: **구현하지 않는다.**

### Addendum 2026-06-04 (epic #4861 종료 전 종합리뷰 — 사실 정정)

원 Decision/§7 본문의 두 진술이 후속 E21 PR 이후 stale/오버클레임이 됨 — dated 본문 재작성 대신 여기서 정정(#5071 추적):

- **§1/Condition 1 "identical `auth_probe_mode: codex_login_status_text`"** — #4868로 supersede: `provider_surface.openai.codex_app_server`는 이제 `auth_probe_mode: codex_account_read_json`(구조화 read-only `account/read`)로 프로브, `exec` surface(`provider_surface.openai.subprocess_cli`)만 `codex_login_status_text` 유지. Decision 1 불변(둘 다 `credential_kind: cli_subscription`, CLI가 OAuth 소유, 신규 ToS surface 없음) — "identical"은 자격증명 모델을 가리켰고 그건 여전히 성립, 프로브 메커니즘만 다름.
- **§7 "`host_injects_token: false` … enforced by a CI lint"** — 현재형 오버클레임: catalog 필드는 설정됐으나 CI lint는 아직 미존재(landed control 아닌 Known Follow-up). §7은 "to be enforced by a CI lint (follow-up)"로 읽을 것. §7 `row-terms-of-use` references 추가도 미적용(함께 추적).
