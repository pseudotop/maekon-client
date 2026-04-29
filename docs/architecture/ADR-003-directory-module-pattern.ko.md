[English](./ADR-003-directory-module-pattern.md) | [한국어](./ADR-003-directory-module-pattern.ko.md)

# ADR-003: 대형 소스 파일의 디렉토리 모듈 패턴

**상태**: 채택됨 (Accepted)
**일자**: 2026-02-27
**범위**: 워크스페이스 내 모든 크레이트

---

## 배경

워크스페이스의 여러 소스 파일이 500줄을 초과하여 탐색, 코드 리뷰, 유지보수가 어려워졌다. 서버 측 코드베이스에서는 이미 유사한 패턴(서버 ADR-013: Domain Service Folder Pattern)을 500줄 초과 Python 모듈에 적용하여 5개 도메인에서 긍정적 결과를 얻었다.

이 결정 시점에 식별된 파일 목록:

| 파일 | 줄 수 | 크레이트 |
|------|-------|---------|
| `handlers/automation.rs` | 1,558 | oneshim-web |
| `controller.rs` | 1,465 | oneshim-automation |
| `updater.rs` | 1,418 | oneshim-app |
| `config.rs` | 1,382 | oneshim-core |
| `app.rs` | 1,227 | oneshim-ui |
| `scheduler.rs` | 1,067 | oneshim-app |
| `focus_analyzer.rs` | 859 | oneshim-app |
| `policy.rs` | 815 | oneshim-automation |
| `gui_interaction.rs` | 750 | oneshim-automation |

`main.rs`(726줄)는 제외 — 순차적 DI 배선이 주요 관심사인 바이너리 진입점으로, 분리 시 구성 로직이 분산되어 명확한 책임 경계가 없어진다.

---

## 결정

### 1. 대형 파일을 디렉토리 모듈로 변환

Rust 소스 파일이 **500줄**을 초과하면 단일 파일(`foo.rs`)에서 디렉토리 모듈(`foo/mod.rs` + 하위 파일)로 변환한다.

### 2. `pub use` 재export으로 외부 API 보존

`mod.rs`는 모든 공개 심볼을 재export하여 **기존 import 경로가 변경 없이 컴파일**되도록 한다. 분리 후 하위 소비자가 수정할 필요가 없어야 한다.

```rust
// foo/mod.rs
mod helpers;
mod types;

pub use helpers::*;
pub use types::*;
```

### 3. 내부 항목에 `pub(super)` 사용

디렉토리 내 하위 파일 간 공유하되 외부에 노출하지 않을 항목은 `pub(super)` 가시성을 사용한다.

```rust
// foo/helpers.rs
pub(super) fn require_config_manager(state: &AppState) -> Result<&ConfigManager, ApiError> {
    // ...
}
```

### 4. 테스트는 `mod.rs`에 유지

모든 `#[cfg(test)] mod tests` 블록은 `mod.rs`에 남긴다. 테스트는 모듈의 공개 인터페이스를 자연스럽게 검증하며, 모듈 경계에서 기대 동작의 문서 역할을 한다.

### 5. SOLID 원칙이 줄 수보다 우선

**파일은 줄 수를 초과해서가 아니라 단일 책임 원칙(SRP)을 위반할 때 분리해야 한다.** 하나의 명확한 책임을 가진 잘 구조화된 1,000줄 파일이 관심사가 얽힌 300줄 파일 3개보다 낫다.

500줄 임계값은 **리뷰 신호**이지 분리 트리거가 아니다:
- **>500줄 → SRP 위반 여부 리뷰.** 하나의 응집된 책임이면 그대로 둔다.
- **모든 크기에서 SRP 위반 → 책임 기준으로 분리.** 관련 없는 두 관심사가 있는 200줄 파일도 분리해야 한다.

하위 파일은 기능적 책임 기준으로 구성한다:

- **types/models**: 데이터 구조체, enum, DTO
- **helpers**: 비공개 유틸리티 함수
- **기능 그룹**: 논리적으로 응집된 핸들러/메서드 그룹 (예: `scene.rs`, `execution.rs`, `intent.rs`, `preset.rs`)

### 6. 임계값 및 제외 대상

- **임계값**: 500줄 (소프트 가이드라인 — 리뷰 신호이지 분리 트리거가 아님)
- **우선순위**: SOLID 준수 > 줄 수. 크기만으로 기계적으로 분리하지 않는다.
- **제외**: `main.rs` 등 순차적 구성 로직이 주요 관심사인 바이너리 진입점
- **소급 적용 불가**: 500줄 미만 파일은 선제적으로 분리하지 않는다

---

## 적용된 분리

| 원본 파일 | 대상 구조 | 크레이트 |
|-----------|----------|---------|
| `gui_interaction.rs` | `gui_interaction/{mod, types, crypto, helpers, service}.rs` | oneshim-automation |
| `policy.rs` | `policy/{mod, models, token}.rs` | oneshim-automation |
| `controller.rs` | `controller/{mod, types, intent, preset}.rs` | oneshim-automation |
| `focus_analyzer.rs` | `focus_analyzer/{mod, models, suggestions}.rs` | oneshim-app |
| `scheduler.rs` | `scheduler/{mod, config, loops}.rs` | oneshim-app |
| `updater.rs` | `updater/{mod, github, install, state}.rs` | oneshim-app |
| `config.rs` | `config/{mod, enums, sections}.rs` | oneshim-core |
| `handlers/automation.rs` | `handlers/automation/{mod, helpers, scene, execution}.rs` | oneshim-web |
| `app.rs` | `app/{mod, message, update, view}.rs` | oneshim-ui |

---

## 결과

### 긍정적 효과

- 각 하위 파일이 300줄 미만으로 탐색 및 코드 리뷰가 개선됨
- `cargo test/clippy/fmt`가 로직 변경 없이 계속 통과
- 외부 API 경로가 완전히 보존 — 하위 호환성 깨짐 없음
- 서버 측 ADR-013 폴더 패턴과 일관되어 모노레포 전반의 인지 부하 감소

### 절충

- 파일 수 소폭 증가 (9개 파일 → ~35개 파일)
- 개발자가 `pub(super)` 및 재export 패턴을 이해해야 함
- `mod.rs` 파일에 재export 보일러플레이트 존재

### 위험

- `pub use *` 재export이 나중에 추가되는 항목을 의도치 않게 노출할 수 있음. 코드 리뷰와 내부 항목에 대한 `pub(super)` 규율로 완화.

---

## 관련 문서

- 서버 ADR-013: `server/docs/architecture/ADR-013-domain-service-folder-pattern.md`
- `docs/architecture/ADR-001-rust-client-architecture-patterns.md`
- Repository guardrails — crate summary 섹션에 각 디렉토리 모듈 구조 기술
