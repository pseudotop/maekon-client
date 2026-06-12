[English](./ADR-006-ipc-command-contract.md) | [한국어](./ADR-006-ipc-command-contract.ko.md)

# ADR-006: Tauri IPC Command Contract

**날짜**: 2026-03-08
**상태**: Accepted
**결정자**: MAEKON 팀
**관련**: [ADR-005: Tauri v2 거버넌스](ADR-005-tauri-governance.ko.md)

---

## 컨텍스트

Tauri v2는 `tauri::generate_handler!` 매크로(`.invoke_handler()` 빌더 메서드로 등록)를 통해 Rust 함수를 JavaScript 프론트엔드에 노출한다. 이를 통해 Rust 백엔드(`src-tauri/src/commands.rs`)와 React 프론트엔드 사이에 타입이 지정된 IPC 표면이 만들어진다. 명문화된 계약이 없으면 IPC 호출이 감사, 버저닝, 테스트하기 어려운 암묵적 API가 된다.

이 ADR은 현재 IPC 표면을 문서화하고, 오류 처리 패턴을 정의하며, 파괴적 변경에 대한 버저닝 정책을 수립한다.

---

## 결정

모든 Tauri IPC 커맨드는 `src-tauri/src/commands.rs`에 정의되고, `tauri::generate_handler!`를 통해 `src-tauri/src/main.rs`에 등록된다. 등록된 커맨드의 전체 집합이 권위 있는 IPC 표면이다. 다른 엔트리포인트는 존재하지 않는다.

---

## 현재 커맨드 표면

2026-03-08 기준으로 다음 커맨드가 등록되어 있다.

### `get_metrics`

현재 시스템 및 에이전트 리소스 사용량을 반환한다.

**입력**: 없음

**출력**:
```typescript
{
  agent_cpu: number;       // 에이전트 프로세스 CPU 사용률 (%)
  agent_memory_mb: number; // 에이전트 프로세스 메모리 (MB)
  system_cpu: number;      // 전체 시스템 CPU 사용률 (%)
  system_memory_used_mb: number;
  system_memory_total_mb: number;
}
```

**오류**: String — `sysinfo` 실패 (드묾; 부분 실패 시 0값 반환)

---

### `get_settings`

현재 `AppConfig`를 JSON 오브젝트로 반환한다.

**입력**: 없음

**출력**: 전체 `AppConfig` JSON 오브젝트. shape는 `crates/maekon-core/src/config/mod.rs`의 `AppConfig` 구조체와 일치한다.

**오류**: String — 직렬화 실패 (실제로는 발생하지 않음)

---

### `update_setting`

부분 설정 패치를 적용한다. 허용 목록에 있는 최상위 키만 수락하며, 나머지는 오류를 반환한다.

**입력**:
```typescript
config_json: string  // 부분 설정 오브젝트를 포함하는 JSON 문자열
```

**허용된 최상위 키** (서버 측에서 적용; 다른 키는 오류 반환):
- `monitoring`
- `capture`
- `notification`
- `web`
- `schedule`
- `telemetry`
- `privacy`
- `update`
- `language`
- `theme`

패치는 현재 설정에 깊게 병합된다. 패치에 없는 키는 보존된다.

**출력**: `void`

**오류**: String — 잘못된 JSON, 허용되지 않는 키, 또는 설정 직렬화 실패

**보안 참고**: `server`, `sandbox`, `ai_provider`, `file_access`, `grpc` 같은 키는 WebView에서 수정할 수 없다. 이 키들은 `config.json`을 직접 편집해야만 변경할 수 있다(설정 디렉토리에 대한 OS 수준 접근 필요).

---

### `get_update_status`

자동 업데이터의 현재 상태를 반환한다.

**입력**: 없음

**출력**: `phase` 필드가 있는 JSON 오브젝트. 업데이트가 비활성화된 경우: `{"phase": "Disabled", "message": "Updates disabled"}`. 활성화된 경우 phase는 업데이터 상태 머신을 반영한다 (예: `Idle`, `Checking`, `Available`, `Downloading`, `Ready`).

**오류**: String — 직렬화 실패

---

### `approve_update`

보류 중인 업데이트 설치를 트리거한다. 이 커맨드를 호출하기 전에 사용자가 확인해야 한다.

**입력**: 없음

**출력**: `void`

**오류**: String — 보류 중인 업데이트 없음, 또는 업데이트 액션 채널이 닫힘

---

### `defer_update`

보류 중인 업데이트를 다음 확인 간격으로 연기한다.

**입력**: 없음

**출력**: `void`

**오류**: String — 업데이트 액션 채널이 닫힘

---

### `get_automation_status`

자동화 컨트롤러가 설정되어 활성 상태인지 여부를 반환한다.

**입력**: 없음

**출력**: `boolean` — `AutomationController`가 초기화되어 있으면 `true`

**오류**: String (실제로는 없음)

---

## 오류 처리 패턴

모든 커맨드는 `Result<T, String>`을 반환한다. 오류는 JavaScript 소비를 위해 문자열로 직렬화된다. 프론트엔드는 null이 아닌 오류 문자열을 실패로 처리하고 사용자에게 표시하거나 로깅해야 한다.

```typescript
// 관용적인 프론트엔드 사용 예시
const result = await invoke<MetricsResponse>('get_metrics');
// Tauri는 Err(_) 시 throw — try/catch로 감싼다
```

인프라 오류(sysinfo 실패, 설정 I/O 오류)는 커맨드 경계에서 문자열로 래핑된다. 현재 버전에서는 구조화된 오류 코드를 사용하지 않는다.

---

## 보안 모델

IPC 커맨드는 임베디드 WebView 프론트엔드에서만 호출 가능하다. Tauri 보안 모델은 프로세스 수준에서 이를 강제한다 — 외부 프로세스, 네트워크 요청, 브라우저 확장 프로그램은 이 커맨드들을 호출할 수 없다.

`update_setting` 커맨드는 JavaScript가 아니라 Rust 계층에서 allowlist를 적용한다. 프론트엔드가 raw IPC 메시지를 구성해도 이 검사를 우회할 수 없다.

민감한 설정 섹션(`server`, `grpc`, `ai_provider`, `sandbox`, `file_access`)은 의도적으로 allowlist에서 제외된다. 이것들은 사용자가 UI에서 제어하는 것이 아니라 관리자가 파일 시스템을 통해 설정하는 필드다.

---

## 버저닝 정책

### 비파괴적 변경 (버전 bump 없이 허용)

- 기존 커맨드 출력에 새 필드 추가 (Rust 측에 `#[serde(default)]` 포함)
- 핸들러에 새 커맨드 추가 (새 커맨드는 추가적)
- 입력/출력 타입 변경 없이 내부 구현 변경

### 파괴적 변경 (메이저 버전 bump 필요)

- 커맨드 제거
- 커맨드 이름 변경
- 기존 커맨드의 입력 타입을 하위 호환되지 않는 방식으로 변경
- 기존 커맨드의 출력 타입을 하위 호환되지 않는 방식으로 변경
- 지원 중단 기간 없이 커맨드 출력에서 필드 제거

파괴적 변경이 필요한 경우:
1. `Cargo.toml` 워크스페이스 `version`에서 메이저 버전을 증가한다.
2. 새 커맨드 표면으로 이 ADR을 업데이트한다.
3. `CHANGELOG.md`에 `BREAKING CHANGE` 항목을 추가한다.
4. 릴리스 전에 다운스트림 팀에 통보한다.

---

## 새 커맨드 추가

새 IPC 커맨드를 추가하려면:

1. `src-tauri/src/commands.rs`에 `#[command]`와 함께 함수를 정의한다.
2. `src-tauri/src/main.rs`의 `tauri::generate_handler![]` 호출에 함수를 추가한다.
3. 새 커맨드의 입력, 출력, 오류 계약으로 이 ADR을 업데이트한다.
4. 프론트엔드에서 응답 shape에 대한 TypeScript 타입 선언을 추가한다.

파일 시스템 경로, 프로세스 목록, 네트워크 설정을 프론트엔드에 노출하는 커맨드는 추가하지 않는다. 해당 데이터는 기존 `get_settings` / `update_setting` 패턴을 통해 라우팅한다.
