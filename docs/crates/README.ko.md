[English](./README.md) | [한국어](./README.ko.md)

# 크레이트 구현 문서

ONESHIM Rust 클라이언트의 현재 15-패키지 워크스페이스(`crates/` 하위 14개 + `src-tauri` 바이너리 패키지; `cargo metadata --no-deps` 기준) 상세 구현 레퍼런스입니다.

## 크레이트 의존성 그래프

```
┌──────────────────────────────────────────────────────────────────────┐
│      src-tauri/ (패키지: oneshim-app, composition root)             │
│  런타임 와이어링, 스케줄러, desktop lifecycle, web server startup   │
└──────────────────────────────────────────────────────────────────────┘
          │
          ├── 런타임 어댑터: analysis / audio / automation / embedding / monitor
          ├── 런타임 어댑터: network / storage / suggestion / vision / web
          └── 공유 계약: oneshim-core / oneshim-api-contracts

oneshim-core
  └── 도메인 모델, 설정, 에러, cross-crate 포트

oneshim-api-contracts
  └── oneshim-web 및 oneshim-network가 사용하는 공유 HTTP/integration DTO 계약 크레이트

런타임 어댑터 베이스라인 (일반 의존성)
  ├── oneshim-analysis   -> oneshim-core
  ├── oneshim-audio      -> oneshim-core
  ├── oneshim-automation -> oneshim-core
  ├── oneshim-embedding  -> oneshim-core
  ├── oneshim-monitor    -> oneshim-core
  ├── oneshim-storage    -> oneshim-core
  ├── oneshim-suggestion -> oneshim-core
  ├── oneshim-vision     -> oneshim-core
  ├── oneshim-network    -> oneshim-core + oneshim-api-contracts
  └── oneshim-web        -> oneshim-core + oneshim-api-contracts

Out-of-process 격리 실행기 (oneshim-app이 spawn)
  └── oneshim-sandbox-worker -> oneshim-core
      (standalone binary; stdin SandboxRequest JSON → stdout SandboxResponse JSON
       플랫폼 sandbox 하에서 — Windows Job Object, Linux seccomp+Landlock, macOS App Sandbox)

툴링 패키지
  └── oneshim-lint (워크스페이스 내부 lint/test 헬퍼, 런타임 그래프에 포함되지 않음)
```

## 활성 워크스페이스 패키지

| 패키지 | 위치 | 역할 | 문서 |
|--------|------|------|------|
| **oneshim-core** | `crates/oneshim-core` | Foundation 레이어: 모델, 포트, 에러, 설정 | [상세](./oneshim-core.ko.md) |
| **oneshim-api-contracts** | `crates/oneshim-api-contracts` | web/integration DTO의 공유 전송 계약 SSOT | [상세](./oneshim-api-contracts.md) |
| **oneshim-audio** | `crates/oneshim-audio` | 오디오 캡처, STT providers, 모델 다운로드 헬퍼 | 전용 문서 작성 예정 |
| **oneshim-monitor** | `crates/oneshim-monitor` | 시스템 모니터링 어댑터 | [상세](./oneshim-monitor.ko.md) |
| **oneshim-vision** | `crates/oneshim-vision` | Edge 캡처, OCR, 프라이버시 필터, 접근성 헬퍼 | [상세](./oneshim-vision.ko.md) |
| **oneshim-network** | `crates/oneshim-network` | HTTP/SSE/WebSocket/gRPC/network 어댑터 | [상세](./oneshim-network.ko.md) |
| **oneshim-storage** | `crates/oneshim-storage` | SQLite 영속, retention, 동기화 추출/병합 | [상세](./oneshim-storage.ko.md) |
| **oneshim-suggestion** | `crates/oneshim-suggestion` | 제안 큐, 이력, 피드백 파이프라인 | [상세](./oneshim-suggestion.ko.md) |
| **oneshim-web** | `crates/oneshim-web` | 로컬 웹 전달 레이어: Axum + 임베디드 frontend | [상세](./oneshim-web.ko.md) |
| **oneshim-automation** | `crates/oneshim-automation` | 정책, sandbox, 감사, GUI 자동화 실행 | [상세](./oneshim-automation.ko.md) |
| **oneshim-analysis** | `crates/oneshim-analysis` | 분석 파이프라인, 코칭, regime/tiered-memory 로직 | 전용 문서 작성 예정 |
| **oneshim-embedding** | `crates/oneshim-embedding` | 로컬 임베딩 provider 어댑터 | 전용 문서 작성 예정 |
| **oneshim-lint** | `crates/oneshim-lint` | 워크스페이스 툴링 및 언어/lint 헬퍼 | 전용 문서 작성 예정 |
| **oneshim-sandbox-worker** | `crates/oneshim-sandbox-worker` | Out-of-process 샌드박스 자동화 action 실행기 (stdin JSON → stdout JSON) | 전용 문서 작성 예정 |
| **oneshim-app** | `src-tauri` | 바이너리 패키지 / composition root / desktop 런타임 오케스트레이션 | [상세](./oneshim-app.ko.md) |

## 역사적 패키지 문서

| 패키지 | 상태 | 문서 |
|--------|------|------|
| **oneshim-ui** | iced → Tauri 마이그레이션 시 워크스페이스에서 제거; 역사적 레퍼런스로만 유지 | [역사적](./oneshim-ui.ko.md) |

## 아키텍처 원칙

### Hexagonal Architecture (Ports & Adapters)

- **Core**: `oneshim-core`가 모든 포트(trait)와 도메인 모델 정의
- **전송 계약**: `oneshim-api-contracts`가 공유 전달/integration DTO 보유
- **어댑터**: 런타임 어댑터 크레이트는 `oneshim-core`에 의존; 전달/네트워크 크레이트는 `oneshim-api-contracts`에도 의존 가능
- **Composition root**: `oneshim-app` (`src-tauri/` 내부 패키지)만 여러 런타임 어댑터를 직접 집계

### Cross-Crate 통신 규칙

1. 일반 런타임 의존성은 `oneshim-core`를 대상으로 하거나, 전송 DTO 공유 시 `oneshim-api-contracts`.
2. `oneshim-app`(`src-tauri/`)만 여러 어댑터를 직접 집계 가능.
3. 현재 non-core 일반 의존성 예외: `oneshim-network -> oneshim-api-contracts`, `oneshim-web -> oneshim-api-contracts`; `oneshim-audio`는 core-only 어댑터.
4. dev/build-only 의존성은 별도 추적되며 런타임 아키텍처 엣지로 취급되지 않음.
5. CI가 `scripts/check-architecture-deps.sh`로 현재 런타임 베이스라인을 강제.

### DI 패턴

- `Arc<dyn T>` 생성자 주입
- DI 프레임워크 없음; 수동 와이어링
- `src-tauri/src/main.rs`, `src-tauri/src/setup.rs`, 그리고 `app_runtime_launch.rs`, `agent_runtime.rs`, `web_server_runtime.rs` 같은 app-layer builder에서 와이어링

### 2-레이어 자동화 액션 모델

- **AutomationIntent** (서버 → 클라이언트): 고수준 의도 (예: ClickElement, TypeIntoElement)
- **AutomationAction** (클라이언트 내부): 저수준 액션 (예: MouseMove, MouseClick, KeyType)
- **IntentResolver**: 의도를 실행 가능 액션 시퀀스로 변환 (OCR + LLM 보조)

## 테스트 및 품질 상태

본 파일은 테스트 카운트, 경고 카운트, pass/fail 상태 등 하드코딩된 총계를 의도적으로 피합니다. 현재 GitHub Actions run 페이지를 live source of truth 로 사용하세요.

## 참조

- [문서 인덱스](../README.ko.md)
- [ADR-001: Rust Client Architecture Patterns](../architecture/ADR-001-rust-client-architecture-patterns.ko.md)
- [ADR-002: OS GUI Interaction Boundary and Runtime Split](../architecture/ADR-002-os-gui-interaction-boundary.ko.md)
- [ADR-009: Client Architecture Baseline](../architecture/ADR-009-client-architecture-baseline.ko.md)
- [CONTRIBUTING.md](../../CONTRIBUTING.md) - 기여 워크플로우
- [Contributing Guide](../../CONTRIBUTING.md)
- [Code of Conduct](../../CODE_OF_CONDUCT.md)
- [Security Policy](../../SECURITY.md)
