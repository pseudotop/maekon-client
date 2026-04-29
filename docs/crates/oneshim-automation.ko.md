[English](./oneshim-automation.md) | [한국어](./oneshim-automation.ko.md)

# oneshim-automation

자동화 제어 크레이트. 정책 기반 명령 실행, 감사 로깅, OS 네이티브 샌드박스, UI 자동화 의도 해석, 워크플로우 프리셋을 담당한다.

## 개요

서버에서 수신한 자동화 명령을 정책 토큰 검증 후 실행하며, 모든 명령은 감사 로그에 기록된다.
2-레이어 액션 모델을 사용: **AutomationIntent** (서버→클라이언트 고수준 의도) → **AutomationAction** (클라이언트 내부 저수준 액션).

## 디렉토리 구조

```
oneshim-automation/src/
├── lib.rs              # 크레이트 루트 (16개 모듈)
├── action_dispatcher.rs # AutomationActionDispatcher — 액션 실행 포트
├── audit.rs            # AuditLogger — 감사 로깅 (14개 메서드)
├── controller/         # AutomationController — 디렉토리 모듈 (ADR-003)
│   ├── mod.rs          # 구조체 + 빌더 + 검증자 + 재export + 테스트
│   ├── types.rs        # AutomationCommand, CommandResult, WorkflowResult 등
│   ├── intent.rs       # 의도 실행 + 씬 분석 메서드
│   └── preset.rs       # 워크플로우/프리셋 실행 메서드
├── gui_interaction/    # GUI V2 세션 + nonce 재생 방지 + TTL (ADR-002 M3)
├── input_driver.rs     # NoOpInputDriver — 테스트/기본 입력 드라이버
├── intent_planner.rs   # 의도 플래닝 (파싱 + 시퀀싱)
├── intent_resolver.rs  # IntentResolver + IntentExecutor — 의도 해석 + 실행
├── local_llm.rs        # LocalLlmProvider — 로컬 LLM (규칙 기반)
├── overlay.rs          # 오버레이 드라이버 인터페이스 (src-tauri의 MagicOverlayDriver가 구현)
├── policy/             # PolicyClient — 디렉토리 모듈 (ADR-003)
│   ├── mod.rs          # 공개 API + 재export + 테스트
│   ├── models.rs       # AuditLevel, ExecutionPolicy, PolicyCache, ProcessOutput
│   └── token.rs        # 토큰 생성, 파싱, 서명 검증, HMAC
├── presets.rs          # builtin_presets() — 내장 워크플로우 10개
├── resolver.rs         # 정책 → 샌드박스 프로필 리졸버 (순수 함수 3개)
├── sandbox/            # OS 네이티브 커널 샌드박스
│   ├── mod.rs          # create_platform_sandbox() 팩토리
│   ├── noop.rs         # NoOpSandbox — 비활성 시 기본
│   ├── linux.rs        # LinuxSandbox — seccomp + namespaces
│   ├── macos.rs        # MacOsSandbox — sandbox-exec + App Sandbox
│   └── windows.rs      # WindowsSandbox — Job Objects + AppContainers
└── error.rs            # AutomationError (ADR-019 typed code)
```

## 모듈

### `controller/` — AutomationController (디렉토리 모듈)

정책 검증 + 명령 실행 + 감사 로깅 + 샌드박스 관리의 중심 제어기. ADR-003에 따라 `types.rs`(열거형/구조체), `intent.rs`(의도 실행), `preset.rs`(워크플로우 실행)로 분리.

- `AutomationController::new(sandbox, sandbox_config)` — 생성자 (`Arc<dyn Sandbox>` + `SandboxConfig`)
- `set_intent_executor(executor)` — IntentExecutor 주입
- `set_action_dispatcher(dispatcher)` — 액션 실행 어댑터 교체
- `execute_command(command)` — 정책 검증 → 감사 로그 → 액션 디스패치 → 결과 반환
- `execute_intent(intent, config)` — 고수준 의도 실행 (IntentExecutor 위임)
- `resolve_for_command(command)` — 정책 기반 동적 SandboxConfig 결정
- 기본 비활성 (`enabled: false`), `set_enabled()` 로 활성화
- `tokio::time::timeout` 기반 실행 타임아웃

### `policy/` — PolicyClient (디렉토리 모듈)

서버 정책 동기화 + 명령 검증 + 프로세스 허가 관리. ADR-003에 따라 `models.rs`(데이터 타입)와 `token.rs`(토큰 연산)로 분리.

- `ExecutionPolicy` — 정책 ID, 프로세스 이름, 바이너리 해시, 인자 패턴, sudo 필요 여부, 감사 레벨
  - `sandbox_profile: Option<SandboxProfile>` — 서버 오버라이드
  - `allowed_paths: Vec<String>` — 정책별 허용 경로
  - `allow_network: Option<bool>` — 네트워크 오버라이드
  - `require_signed_token: bool` — 토큰 서명 필수 여부
- `AuditLevel` enum: None, Basic, Detailed, Full
- `PolicyCache` — 정책 목록 + TTL 캐시 (기본 5분)
- `issue_command_token(policy_id)` — 정책 계약에 맞는 토큰 발급
- `issue_command_token_for_command(policy_id, cmd)` — 명령 스코프 토큰(`h{command_hash}` 세그먼트) 발급
- `validate_command()` — 토큰 형식 + nonce + 캐시 TTL + 정책 매칭 + 재사용 방지 + 선택적 서명 검증 + 선택적 명령 스코프 해시 검증
- `validate_args()` — glob 패턴 기반 인자 검증 (`*` 와일드카드)
- `is_process_allowed()` — HashSet 기반 빠른 프로세스 허가 조회
- 토큰 계약: `docs/contracts/policy-token-contract.md`

#### 정책 토큰 변형

- 비서명: `{policy_id}:{nonce}`
- 비서명 명령 스코프: `{policy_id}:{nonce}:h{command_hash}`
- 서명: `{policy_id}:{nonce}:{signature}`
- 서명 명령 스코프: `{policy_id}:{nonce}:h{command_hash}:{signature}`

#### 검증 의미론 (fail-closed)

`validate_command()`는 아래 검증을 순차 통과한 경우에만 명령을 허용한다.

1. 토큰 파싱 + nonce 형식 검증.
2. 정책 캐시 TTL 유효성 검증.
3. 활성 캐시 내 정책 ID 매칭.
4. `require_signed_token=true`일 때 서명 검증.
5. 토큰에 `h{command_hash}`가 있으면 명령 스코프 해시 검증.
6. TTL 범위 내 validated token 캐시 기반 재사용 차단.

서명 검증은 `ONESHIM_POLICY_TOKEN_SIGNING_SECRET`를 사용한다. 정책이 서명을 요구하는데 시크릿이 없으면 즉시 실패 폐쇄(fail-closed)한다.

### `audit.rs` — AuditLogger

로컬 VecDeque 버퍼 + 배치 전송 감사 로그. 비파괴 조회 메서드 포함.

#### 타입

```rust
pub enum AuditStatus { Started, Completed, Failed, Denied, Timeout }

pub struct AuditEntry {
    pub entry_id: String,
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub command_id: String,
    pub action_type: String,
    pub status: AuditStatus,
    pub details: Option<String>,
    pub execution_time_ms: Option<u64>,
}
```

#### 메서드 (14개)

| 분류 | 메서드 | 설명 |
|------|--------|------|
| 기본 로깅 | `log_start()` | 명령 시작 기록 |
| | `log_complete()` | 명령 완료 기록 |
| | `log_denied()` | 정책 거부 기록 |
| | `log_failed()` | 실행 실패 기록 |
| 조건부 로깅 | `log_start_if(level, ...)` | AuditLevel::None이면 스킵 |
| | `log_complete_with_time(level, ..., ms)` | 실행 시간 포함 기록 |
| | `log_timeout(...)` | 타임아웃 기록 |
| 배치 관리 | `has_pending_batch()` | 전송 준비 여부 |
| | `pending_count()` | 대기 중 항목 수 |
| | `drain_batch()` | 배치 크기만큼 꺼내기 |
| | `drain_all()` | 전체 꺼내기 (셧다운 시) |
| 비파괴 조회 | `recent_entries(limit)` | 최근 N개 조회 (API용) |
| | `entries_by_status(status, limit)` | 상태별 필터링 |
| | `stats()` | 통계 집계 (total, success, failed, denied, timeout) |

- 버퍼 오버플로 시 가장 오래된 항목 자동 제거
- 기본 설정: 최대 1000개 버퍼, 50개 배치 크기

### `resolver.rs` — 정책 → 샌드박스 리졸버

순수 함수 3개 (상태 없음, 테스트 용이):

| 함수 | 설명 |
|------|------|
| `resolve_sandbox_profile(policy)` | AuditLevel → SandboxProfile 계단식 매핑 |
| `resolve_sandbox_config(policy, base)` | 정책 기반 동적 SandboxConfig 생성 |
| `default_strict_config(base)` | 정책 없는 명령용 Strict 설정 |

#### AuditLevel → SandboxProfile 매핑

```
AuditLevel::None     → SandboxProfile::Permissive
AuditLevel::Basic    → SandboxProfile::Standard
AuditLevel::Detailed → SandboxProfile::Strict
AuditLevel::Full     → SandboxProfile::Strict
```

- `requires_sudo=true` 이면 Permissive → Standard 승격
- 서버 `sandbox_profile` 오버라이드 우선 적용

### `intent_resolver.rs` — IntentResolver + IntentExecutor

고수준 의도(AutomationIntent)를 저수준 액션(AutomationAction) 시퀀스로 변환하고 실행.

- `IntentResolver` — UI 요소 탐색 → 좌표 계산 → 액션 변환
  - OCR 기반 요소 탐색 (`ElementFinder`)
  - LLM 기반 의도 해석 (`LlmProvider`)
  - 신뢰도 검증 + 재시도 로직 (`IntentConfig`)
- `IntentExecutor` — 변환된 액션 순차 실행 + 결과 검증
  - `execute_intent(intent, config)` → `IntentResult`
  - 실행 후 텍스트 확인 (`verify_after_action`)
  - 재시도 (`max_retries`, `retry_interval_ms`)

### `presets.rs` — 내장 워크플로우 프리셋

`builtin_presets()` 함수가 10개 내장 프리셋을 반환. 플랫폼별 키 매핑 자동 적용.

#### 생산성 프리셋 (4개)

| ID | 이름 | 단계 |
|----|------|------|
| `save-file` | 파일 저장 | `ExecuteHotkey(["Cmd/Ctrl", "S"])` |
| `undo` | 실행 취소 | `ExecuteHotkey(["Cmd/Ctrl", "Z"])` |
| `select-all-copy` | 전체 선택 후 복사 | `Cmd/Ctrl+A` → 200ms → `Cmd/Ctrl+C` |
| `find-replace` | 찾기/바꾸기 | `ExecuteHotkey(["Cmd/Ctrl", "H"])` |

#### 앱 관리 프리셋 (3개)

| ID | 이름 | 단계 |
|----|------|------|
| `switch-next-app` | 다음 앱 전환 | `Cmd/Alt+Tab` |
| `close-window` | 현재 창 닫기 | `Cmd/Ctrl+W` |
| `minimize-all` | 전체 최소화 | macOS: `Cmd+Option+H+M` / Win: `Win+D` |

#### 워크플로우 프리셋 (3개)

| ID | 이름 | 단계 |
|----|------|------|
| `morning-routine` | 업무 시작 | `ActivateApp(Mail)` → 2s → `Calendar` → 2s → `VSCode` |
| `meeting-prep` | 회의 준비 | `ActivateApp(Zoom)` → 1s → `Notes` |
| `end-of-day` | 업무 종료 | `Cmd/Ctrl+S` → 1s → `Cmd/Ctrl+Q` |

**헬퍼 함수:**
- `platform_modifier()` — macOS: `"Cmd"`, 기타: `"Ctrl"`
- `platform_alt_modifier()` — macOS: `"Cmd"`, 기타: `"Alt"`

### `sandbox/` — OS 네이티브 커널 샌드박스

`create_platform_sandbox()` 팩토리 함수로 플랫폼별 샌드박스 생성.

| 플랫폼 | 구현 | 기술 |
|--------|------|------|
| `config.enabled=false` | `NoOpSandbox` | 패스스루 (제한 없음) |
| Linux | `LinuxSandbox` | seccomp + namespaces |
| macOS | `MacOsSandbox` | sandbox-exec + App Sandbox |
| Windows | `WindowsSandbox` | Job Objects + AppContainers |
| (미지원) | `NoOpSandbox` (폴백) | 경고 로그 + 패스스루 |

### `input_driver.rs` — NoOpInputDriver

테스트/기본 입력 드라이버. `InputDriver` trait 구현체로 모든 액션을 로그만 남기고 무시.

### `local_llm.rs` — LocalLlmProvider

로컬 LLM/규칙 기반 의도 해석. `LlmProvider` trait 구현체. 외부 API 없이 규칙 매칭으로 동작.

## 의존성

```
oneshim-automation → oneshim-core (CoreError, 모델, 포트 trait)
```

## 보안

- **정책 토큰 필수**: 모든 자동화 명령은 서버 발급 정책 토큰 필요
- **서명 토큰 지원**: 서명 정책은 SHA-256 서명(`ONESHIM_POLICY_TOKEN_SIGNING_SECRET`) 필수
- **명령 스코프 바인딩**: 선택적 `h{command_hash}`로 토큰을 특정 명령 스코프에 묶음
- **재사용 방지**: 정책 캐시 TTL 내 토큰 1회성 보장
- **바이너리 해시 검증**: `ExecutionPolicy.process_hash`로 변조 감지
- **인자 패턴 제한**: glob 패턴으로 허용 인자 제한
- **OS 네이티브 샌드박스**: 커널 수준 격리 (seccomp, sandbox-exec, Job Objects)
- **정책 → 샌드박스 자동 바인딩**: AuditLevel에 따라 SandboxProfile 자동 결정
- **실행 타임아웃**: `tokio::time::timeout` 기반 강제 종료
- **감사 로그 기록**: 모든 실행/거부/실패/타임아웃이 감사 로그에 기록
- **기본 비활성**: `AutomationController`는 기본 비활성 상태
- **Privacy Gateway**: 외부 데이터 전송 시 PII 필터 + 민감 앱 차단 + 동의 검증

## 테스트

| 모듈 | 테스트 수 | 설명 |
|------|----------|------|
| controller | 6 | action/result 직렬화, 의도 실행, 타임아웃 |
| policy | 7 | 정책 직렬화, 인자 검증, 정책 업데이트, 샌드박스 필드 |
| audit | 7 | 로그/드레인, 버퍼 오버플로, 배치 부분 추출, 직렬화, 비파괴 조회, 통계 |
| resolver | 5 | 프로필 매핑, sudo 승격, 경로 병합, strict 기본값, 서버 오버라이드 |
| presets | 3 | 프리셋 로드, 플랫폼 키 매핑, 단계 검증 |
| sandbox | 3 | 팩토리 생성, NoOp 패스스루, 기능 보고 |
| intent_resolver | 2 | 의도 해석, 액션 변환 |
| **Total** | **33** | - |
