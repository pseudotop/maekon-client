[English](./market-positioning-references.md) | [한국어](./market-positioning-references.ko.md)

# 시장 포지셔닝 레퍼런스

> 최종 갱신: 2026-06-18

## 목적

본 문서는 Maekon이 운영하는 **공개 시장 카테고리**, 2026년에 동일 문제 공간에 진입한 가장 가까운 비교 제품, 그리고 Maekon이 차별화하는 4축을 기록한다. README, 랜딩 카피, 투자자 brief, 외부 메시지의 정본 포지셔닝 레퍼런스다.

본 문서는 ADR이 **아니다** — 아키텍처 결정이 아니라 시장 맥락을 담는다. 이 포지셔닝에서 파생되는 아키텍처 축은 ADR 레지스트리(`docs/architecture/`)에 별도로 존재한다.

## 문제 공간

**Ambient AI + 화면 맥락 이해** — 사용자의 화면, focus, 활동 흐름을 관찰하고, 자연 지시("이것 요약해", "저것 정리", "여기서 뭐야")를 구조화된 후보·행동으로 전환하는 AI.

2026년 두 주요 actor가 진입:

| Actor | 제품 | 공개 | 표면 |
|---|---|---|---|
| Google DeepMind | **AI Pointer** (Gemini 기반) | 2026-05 | Chrome (Gemini), Googlebook (Magic Pointer), Google Labs (Disco), Google AI Studio |
| OpenAI | **Codex Chronicle** (Recall-like memory) | 2026-04 | macOS only, ChatGPT Pro 구독 (opt-in research preview) |

Source pointer:
- DeepMind AI Pointer: https://deepmind.google/blog/ai-pointer/
- OpenAI Codex Chronicle: https://developers.openai.com/codex/memories/chronicle

## DeepMind AI Pointer — 4 Design Principle (참조)

DeepMind는 AI Pointer의 4가지 design principle을 명시한다 (broader 카테고리에 동일하게 적용 가능):

1. **Maintain the Flow** — 모든 앱에서 작동, 사용자가 AI 사용을 위해 워크플로우를 "우회"하지 않게 함
2. **Show and Tell** — 사용자가 가리키는 대상 주변 시각·의미 맥락 자동 capture
3. **The power of "this/that"** — 자연 shorthand 지시 (맥락 재타이핑 불필요)
4. **Pixels → Actionable Entities** — 픽셀을 시스템이 행동할 수 있는 구조화 entity로 변환

인용: *"AI capabilities should work across all apps, not force users into 'AI detours' between them."*

Maekon은 이 원칙들을 work-signal layer의 **목표 경험**으로 채택하되, 아래 4 운영 축에서 차별화한다.

## OpenHuman (tinyhumansai) — 최근접 local-first 비교 대상 (2026-07-29 스캔)

**제품**: OpenHuman — Tauri+Rust+SQLite local-first "개인 AI 슈퍼 인텔리전스" (2026-05 출시, GPL-3.0). Maekon과 스택·카테고리가 가장 근접한 비교 대상이며, 컨텍스트 획득 축은 정반대다(OpenHuman은 100+ OAuth 커넥터의 20분 pull 루프로 **계정을 읽고**, Maekon은 패시브 로컬 캡처로 **행동을 관찰한다**).

**신호 품질 판정 (지표 인용 전 필독)**:

- 스타/트렌딩 지표는 커뮤니티 검증이 아니다. 전수 검증(2026-07-29): 관련 Hacker News 제출 전체(메인테이너 Show HN + 런칭 당일 서드파티 포함 5건)가 2~4포인트·코멘트 합계 ~1건으로 사망; 인덱싱된 Reddit 스레드 0; 메인테이너 외 유기적 X 반응 0. 영문 "커버리지" 대부분은 트렌딩 순위를 메아리치는 SEO 콘텐츠 팜이다. 성장은 비영어권 채널 + 트렌딩→SEO→스타 증폭 루프로 설명된다. **스타 수·트렌딩 순위·Product Hunt 순위를 수요/제품 증거로 인용하지 말 것.**
- 유효 증거 = hands-on 리뷰 2건뿐: 요즘IT(직접 사용; yozm.wishket.com/magazine/detail/3870)와 OpenAIToolsHub(14일 셀프호스팅). 두 리뷰의 수렴점: 단일 최고 호평 속성은 **inspectable memory vault**("볼트를 열어 에이전트가 아는 것을 정확히 볼 수 있다 — 아무도 못 하는 것"); 일관된 미달은 안정성과 속도.
- 이슈 트래커의 실사용자 신호(최근 트래픽 ~83%는 코어팀 작업 티켓; 최다 반응 커뮤니티 이슈 기준): 1위 불만은 **"local-first" 제품의 강제 온라인 계정**; 사용자 코드 감사 2건이 무동의 자사 백엔드 phone-home과 호스팅 애그리게이터(Composio) 경유 OAuth 핸드셰이크를 적발했다. 해당 스레드 인용: *"'local-first'와 'connected to your tools'는 반대 방향으로 당기는데, 마케팅 카피가 그 이음새를 항상 발라 덮는다."*

**Maekon에 검증되는 것** (이미 내린 결정의 외부 증거):

1. **볼트 베팅** — ADR-033의 사용자 소유 마크다운 미러는 hands-on 리뷰어가 가장 호평하는 속성에 착지한다 — 근거는 스타 수가 아니라 실사용 신호다.
2. **계정 불요** — OpenHuman 커뮤니티의 최대 불만이 정확히 Maekon이 거부한 축이다: Maekon의 standalone 성공 경로는 온라인 계정·OAuth 폭·애그리게이터 중개를 요구하지 않는다(MK-EXT-01: first-party read-only 커넥터만).
3. **Egress 정직성** — OpenHuman의 신뢰 붕괴는 사용자 감사가 적발한 침묵 phone-home에서 왔다; Maekon의 egress ledger + receipt-only/비-PII 텔레메트리 + ADR-033 §3.4 클라우드-경로 ledger 기록이 선제적 공개 답이다. 감사당하기를 기다리지 말고 공개 표면에 명시할 것.

Source pointers: github.com/tinyhumansai/openhuman (신뢰 발견은 이슈 #1977, #2020, #2422).

## Maekon의 4 차별화 축

| 축 | DeepMind AI Pointer | OpenAI Codex Chronicle | **Maekon** |
|---|---|---|---|
| **기본 데이터 경로** | Cloud-bound (Gemini) | Cloud-bound (OpenAI 서버가 screenshot 처리) | **기본 local-first**, on-device. 클라우드 round-trip은 opt-in. |
| **감사·추적** | 공식 명시 없음 | Memory가 디스크에 **unencrypted** 저장 | **Source-first 감사** — 모든 신호에 origin, retention, PII filter trace |
| **자동화 경계** | 자연 지시 → **direct action** | Memory만 (Codex가 행동) | 자연 지시 → **next-action candidates** + 명시적 검토/승인 게이트 (policy-gated) |
| **플랫폼 범위** | Chrome / Gemini / Googlebook (Google 생태계) | macOS only / ChatGPT Pro 구독 / EU/UK/CH 미제공 | **3 OS** (macOS, Windows, Linux), Apache-2.0, 생태계 중립 |

## 신뢰 우선 차별화 차원

위 4개 경쟁 축과 별개로, Maekon의 신뢰 우선(trust-first) 강점은 사용자에게 보이는 다섯 가지 차원으로 표현된다. 각 항목은 배경 주장이 아니라 데스크톱 클라이언트가 사용자에게 직접 증명해야 하는 설계 약속이다 — 가시적 출처(provenance), 동의, 감사, 경량 증거:

| 차원 | 사용자가 직접 확인 가능한 것 | 왜 중요한가 |
|---|---|---|
| **검색 신뢰** | 과거 작업 맥락이 출처(프레임·시간 범위·앱/창·소스 스니펫)와 함께 반환되고, 민감 콘텐츠는 인덱스·결과·내보내기에서 마스킹되며, 낮은 신뢰도의 회상은 날조 대신 명확화를 요청한다 | 검색 가능한 과거 맥락은 불투명하지 않고 감사·마스킹 가능할 때만 신뢰할 수 있다 |
| **에이전트 안전 확인** | 캡처된 화면/웹/앱 콘텐츠는 신뢰할 수 없는 맥락으로 취급되어 의도를 덮어쓸 수 없고, 민감 행동(결제·자격증명·파일/메일 변경·파괴적 자동화)은 명확한 행동 요약과 함께 명시적 확인을 요구하며, 허용목록 거부가 가시화된다 | 화면을 읽는 에이전트는 프롬프트 인젝션에 노출되므로 의미 있는 행동에는 사람의 확인이 필요하다 |
| **데이터 통제 가시성** | 보존·내보내기·삭제·외부 송신·제공자 학습 정책이 평이한 언어로 표시되고, 내보내기는 앱/창/OCR 필드를 새니타이즈하며 누락 시 fail-closed 되고, 공유 기본값은 비공개를 유지한다 | 사용자는 아키텍처 문서를 읽지 않고도 자신의 데이터를 통제할 수 있어야 한다 |
| **오디오·주변인 동의** | 오디오/STT는 명시적 동의 전까지 꺼져 있고, 캡처 범위와 외부 STT 송신이 설명되며, 상시 캡처 전 녹음 고지나 주변인 동의 안내가 표시되고, 취소 시 버퍼가 폐기된다 | 주변/회의형 캡처는 동의·고지·보존·삭제로 평가된다 |
| **증거 가독성** | 캡처 경계·포인터 헤일로·클릭 리플이 앱을 가리지 않고 읽히며, Computer Use 커서와 Maekon 커서가 중복 흔적 없이 구분되고, 모션 감소 모드에서 정적 포인터 증거가 유지된다 | 읽기 쉽고 정직한 캡처 증거가 관찰된 내용에 대한 신뢰를 만든다 |

이 차원들은 신뢰 계층의 **목표 경험(target experience)**을 기술하며 Maekon 차별화의 공개 안전(public-safe) 표현이다. 상세 릴리스 게이트 실행 증거는 parent-internal QA 프로세스에서 관리되며 의도적으로 본 공개 문서에 포함하지 않는다.

## 어휘 정합

Maekon 사용자 표면 어휘와 broader 시장 frame 매핑:

| Maekon 표면 어휘 | DeepMind frame (참조) | 동등 의미 |
|---|---|---|
| "next-action candidates" | "Pixels → Actionable Entities" (principle #4) | 관찰된 맥락을 분리된 actionable suggestion으로 변환 |
| "policy-gated action paths" | "Maintain the Flow" + 감사 제약 | suggestion이 검토 경계 안에 머무름 |
| "edge processing" | "Show and Tell" + on-device | 클라우드 round-trip 전 로컬에서 사전 처리 |
| "delta encoding" | (Maekon 고유) | frame 간 변경분만 전송 (대역폭 절약) |

Maekon은 엔터프라이즈 어휘로 **"pointed context → actionable entity"** 라고도 설명할 수 있습니다. 두 표현은 같은 공개 메커니즘을 가리킵니다 — 로컬 업무 신호 + focus timeline + 화면/OCR edge → 검토 가능한 후보 흐름. 표면 어휘는 청중에 따라 다르지만, 제품 경계는 동일한 Apache-2.0 데스크톱 클라이언트입니다.

## 직접 경쟁이 아닌 이유

Maekon은 DeepMind AI Pointer 또는 Codex Chronicle의 head-to-head 대체로 포지셔닝하지 않는다. 각자 동일 문제 공간을 다른 생태계 가정에서 다룬다:

- DeepMind는 경험을 Google의 클라우드 + 브라우저 스택에 묶는다.
- OpenAI Codex Chronicle은 ChatGPT Pro + macOS에 묶고, memory를 unencrypted로 저장한다.
- Maekon의 베팅: **유의미한 사용자·조직 비중이 local-first 기본값, source-first 감사 추적, policy 게이트가 선결되어야 도입 가능하다** — 특히 규제 산업 (금융·제조·헬스케어·공공) 에서.

이는 **카테고리 인접 차별화**이지 직접 경쟁이 아니다.

## Cross-Reference

- Maekon README: `## Why Maekon → Market positioning (2026)` 참조
- 위 공개 source pointer를 이 OSS 문서의 정본 공개 레퍼런스로 유지합니다.

## 갱신 정책

다음 시 본 문서를 refresh:
- ambient AI + 화면 맥락 공간에 새로운 비교 제품 진입
- Maekon의 4축 변경 (예: local-first 기본값 폐기, cloud-only mode 추가)
- DeepMind 또는 OpenAI 공식 stance 변화 (링크 깨짐, 원칙 갱신)

Companion: [market-positioning-references.md](./market-positioning-references.md)
