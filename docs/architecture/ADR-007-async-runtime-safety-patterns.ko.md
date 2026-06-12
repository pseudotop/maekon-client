[English](./ADR-007-async-runtime-safety-patterns.md) | [한국어](./ADR-007-async-runtime-safety-patterns.ko.md)

# ADR-007: 비동기 런타임 안전 패턴

**상태**: Accepted (2026-04-20 Proposed에서 승격; `spawn_blocking` 경계, 서브프로세스 실행, lock-poison 처리 — 세 가지 결정 모두 워크스페이스 전반에 구현됨; `src-tauri/src/feedback_sink/mod.rs:40`에서 참조됨)
**날짜**: 2026-03-09
**범위**: tokio 비동기 런타임을 사용하는 모든 크레이트

> **예시 코드의 CoreError 문법 참고**: 아래 예시는 [ADR-019](./ADR-019-error-code-infrastructure.md) 이전의
> 튜플 변형 문법 `CoreError::Internal(String)`을 사용한다.
> ADR-019 이후에는 타입화된 `code` 필드가 있는 구조체 변형으로 작성해야 한다:
> ```rust
> CoreError::Internal {
>     code: maekon_core::error_codes::InternalCode::Generic,
>     message: format!("..."),
> }
> ```
> 패턴 자체(spawn_blocking 래핑, 서브프로세스 타임아웃, lock-poison map_err)는
> ADR-019에 의해 변경되지 않는다 — 구성 호출 사이트만 새 구조체 형태가 필요하다.
> wire-code 계약은 ADR-019 참조.

---

## 컨텍스트

client-rust 워크스페이스는 tokio 멀티스레드 런타임 위에서 동작한다. 1초 스케줄러 루프(`src-tauri/src/scheduler/`에 정의)는 9개의 모든 백그라운드 루프에서 일관되고 낮은 레이턴시의 태스크 완료를 요구한다.

세 가지 반복적인 문제가 그 레이턴시 보장을 위협한다.

1. **비동기 태스크 내부의 블로킹 I/O** — `maekon-storage`의 `rusqlite`, `maekon-vision`의 `xcap` 화면 캡처, 비동기 함수에서 호출되는 `std::fs` 호출은 전체 작업 시간 동안 tokio 워커 스레드를 블로킹한다. 워커 스레드 풀이 멈추면 관계없는 비동기 태스크들이 그 뒤에 대기하게 된다.

2. **동기 서브프로세스 호출** — `std::process::Command`는 자식 프로세스가 종료될 때까지 호출 스레드를 블로킹한다. macOS의 `osascript` 호출(`maekon-monitor/src/macos.rs`를 통해)과 Linux의 `xdotool`/`xprintidle` 호출(`maekon-monitor/src/linux.rs`를 통해)이 현재 동기 방식이다. 응답 없거나 느린 서브프로세스는 전체 워커 스레드를 멈춘다.

3. **lock-poison 시 패닉** — `Mutex::lock()` 또는 `RwLock::read()`에 대한 `.expect()`는 전체 스폰된 태스크를 통해 패닉을 전파시키고, 조용히 종료시킨다. 서브프로세스 실패와 하드웨어 이상에서도 살아남아야 하는 24/7 데스크톱 에이전트에게는 조용한 태스크 종료가 서비스 저하보다 더 나쁘다.

### 근거 증거

세 커밋이 이 이슈들의 직접적인 계보를 수립한다.

| 커밋 | 날짜 | 경로 | 관련성 |
|------|------|------|--------|
| `1e8c918` | 2026-02-26 | `crates/maekon-monitor/src/macos.rs`, `crates/maekon-monitor/src/linux.rs` | 초기 코드베이스에서 모든 서브프로세스 호출에 `std::process::Command` 도입 |
| `aa03871` | 2026-02-28 | `crates/maekon-vision/src/trigger.rs` | 내부 가변성 리팩터(`&mut self` → `&self`)에서 `Mutex::lock().expect(...)` 를 잠금 패턴으로 도입 |
| `e633ac5` | 2026-03-08 | `crates/maekon-vision/src/trigger.rs`, `crates/maekon-monitor/src/input_activity.rs` | 부분적 unwrap 정리에서 `unwrap()`을 `.expect()`로 교체 — 문서화된 불변식에는 올바르지만 `.expect()`는 lock poison 시 여전히 패닉; 나머지 케이스는 graceful handling이 필요 |

---

## 결정 사항

### 1. 블로킹 I/O 경계 (`spawn_blocking`)

**규칙**: 비동기 컨텍스트 내에서 스레드를 ~1 ms 이상 블로킹할 수 있는 모든 작업은 `tokio::task::spawn_blocking`으로 오프로드해야 한다. 이는 다음에 적용된다.

- `maekon-storage/src/sqlite/`의 모든 `rusqlite` 데이터베이스 메서드
- `maekon-vision/src/capture.rs`의 `xcap::Monitor::capture_image()`를 통한 화면 캡처
- 비동기 함수에서 호출될 때 `tokio::fs` 대신 `std::fs`를 사용하는 파일 시스템 작업

**SQLite 권장 패턴 — `with_conn` 헬퍼**:

```rust
// 동기 Connection을 소유한 구조체에 이 헬퍼를 추가한다
async fn with_conn<F, T>(&self, f: F) -> Result<T, CoreError>
where
    F: FnOnce(&Connection) -> Result<T, CoreError> + Send + 'static,
    T: Send + 'static,
{
    // Arc<Mutex<Connection>>을 복제하여 클로저로 이동시킨다
    let conn = self.conn.clone();
    tokio::task::spawn_blocking(move || {
        let guard = conn.lock().map_err(|e| {
            CoreError::Internal(format!("SQLite lock poisoned: {e}"))
        })?;
        f(&guard)
    })
    .await
    .map_err(|e| CoreError::Internal(format!("spawn_blocking join error: {e}")))?
}
```

호출 측은 이를 얇은 래퍼로 사용한다.

```rust
// 호출 측 — 동기 rusqlite 코드를 클로저 안에 작성한다
let count = self
    .with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .map_err(|e| CoreError::Internal(e.to_string()))
    })
    .await?;
```

**`tokio::sync::Mutex`를 쓰지 않는 이유?** `tokio::sync::Mutex`도 실제 SQL 실행 중에는 기반 시스템 스레드를 블로킹한다. `spawn_blocking` 경계는 블로킹 작업을 tokio가 비동기 워커 풀과 별도로 사이즈를 조정하는 전용 스레드 풀로 옮겨 head-of-line 블로킹을 방지한다.

---

### 2. 서브프로세스 실행 패턴

**규칙**: 비동기 컨텍스트 내에서 실행되는 모든 코드에서 `std::process::Command` 대신 `tokio::process::Command`를 사용한다. 모든 서브프로세스 호출에는 명시적인 타임아웃이 있어야 한다.

**영향 파일**:
- `maekon-monitor/src/macos.rs` — `osascript`, `ioreg` (현재 `std::process::Command` 사용)
- `maekon-monitor/src/linux.rs` — `xdotool`, `xprintidle` (현재 `std::process::Command` 사용)

**마이그레이션 패턴**:

```rust
use tokio::process::Command;
use tokio::time::{timeout, Duration};

// osascript 호출 예시 — 5초 타임아웃 적용
async fn get_active_window_macos() -> Result<Option<WindowInfo>, CoreError> {
    let output = timeout(
        Duration::from_secs(5),
        Command::new("osascript")
            .arg("-e")
            .arg(APPLESCRIPT)
            .output(),
    )
    .await
    .map_err(|_| CoreError::Internal("osascript timed out".into()))?
    .map_err(|e| CoreError::Internal(format!("subprocess failed: {e}")))?;

    if !output.status.success() {
        return Ok(None);
    }
    // ... 출력 파싱
}
```

**기본 타임아웃 값**:

| 컨텍스트 | 타임아웃 |
|---------|---------|
| Monitor 커맨드 (`osascript`, `xdotool`, `ioreg`) | 5초 |
| OCR 서브프로세스 (Tesseract via `maekon-vision`) | 30초 |
| 기타 서브프로세스 | 10초 (기본값) |

타임아웃은 런타임에 설정 변경이 불가하며 각 모듈의 컴파일 타임 상수다. 서브프로세스가 지속적으로 타임아웃된다면 올바른 수정은 타임아웃을 높이는 것이 아니라 네이티브 Rust API로 교체하는 것이다.

---

### 3. Lock Poison 처리

**규칙**: `Mutex::lock()`, `RwLock::read()`, `RwLock::write()`에 대해 절대 `.expect()`나 `.unwrap()`을 사용하지 않는다. lock-poison 오류는 항상 `.map_err()`를 사용해 `CoreError::Internal`로 전파한다.

**현재 위반 사항** (점진적으로 마이그레이션 예정):

| 파일 | 라인 | 위반 |
|------|------|------|
| `crates/maekon-vision/src/trigger.rs` | 88–89 | `.expect("SmartCaptureTrigger state lock was poisoned...")` |
| `crates/maekon-monitor/src/input_activity.rs` | 114–115 | `.expect("InputActivityCollector period_start lock was poisoned")` |

**패턴**:

```rust
// ❌ 잘못됨 — 이전 태스크가 lock을 보유한 채 패닉하면 패닉 발생
let guard = self.state.lock().expect("lock poisoned");

// ✅ 올바름 — graceful 저하; 이벤트를 로깅하고 오류 반환
let guard = self.state.lock().map_err(|e| {
    tracing::error!(
        target: "maekon::runtime",
        "mutex lock poisoned — previous task may have panicked: {e}"
    );
    CoreError::Internal(format!("lock poisoned: {e}"))
})?;
```

**`.expect()`가 허용되는 경우**: 구조적으로 `PoisonError`가 발생할 수 없는 값(예: `AtomicU32`, `AtomicU64`)이나, 폴러블 코드가 절대 변경하지 않는 `Mutex<Vec<_>>` 등 패닉이 불가능한 컨텍스트에서만 `Mutex` guard를 획득하는 경우. 그런 경우에는 `.expect()` 위에 주석으로 불변식을 문서화한다.

**근거**: tokio 태스크가 `Mutex`를 보유한 채 패닉하면 lock이 poison 상태가 된다. 다른 태스크에서 이어지는 `.lock().expect()`는 다시 패닉을 일으켜 실패가 연쇄된다. 24/7로 실행되며 시스템 상태를 모니터링하는 데스크톱 에이전트에게 올바른 동작은 poison된 lock 이벤트를 로깅하고, 현재 작업을 건너뛰고, 다음 tick에서 데이터 수집을 계속하는 것이다. 에이전트는 개별 모니터링 태스크의 부분적 실패에 탄력적이어야 한다.

---

## 결과

### 장점

- Tokio 워커 스레드가 비동기 스케줄링을 위해 자유롭게 유지된다; 블로킹 작업은 `spawn_blocking` 풀로 격리된다.
- 응답 없는 서브프로세스가 설정된 타임아웃을 넘어 워커 스레드를 멈추지 않는다.
- 하나의 패닉하는 태스크가 더 이상 형제 태스크로 lock-poison 실패를 연쇄시킬 수 없다.
- SQLite나 화면 캡처가 느릴 때도 비블로킹 태스크에 대한 1초 스케줄러 레이턴시 보장이 유지된다.

### 단점 / 트레이드오프

- `spawn_blocking`은 SQLite 호출당 하나의 컨텍스트 전환 오버헤드를 추가한다. SQLite 레이턴시가 이미 작업을 지배하므로 허용 가능하다.
- `tokio::process::Command`는 순수 동기 컨텍스트에서는 사용할 수 없다. 비동기 호출자가 아닌 경우에는 작은 비동기 블록을 스폰하거나 비동기 경계에서 호출하도록 재구성해야 한다. 실제로는 영향받는 모든 monitor 함수가 이미 비동기 스케줄러 루프에서 호출된다.
- `with_conn`은 일반 `Connection` 대신 `Arc<Mutex<Connection>>`이 필요하다. 기존 `SqliteStorage` 구현을 검토하고 업데이트해야 한다.

### 마이그레이션 경로

새 코드는 이 ADR이 수락된 날짜부터 이 패턴을 따라야 한다.

기존 위반은 다음 우선순위 순서로 점진적으로 마이그레이션된다.

1. **높음** — `maekon-monitor/src/macos.rs` 및 `maekon-monitor/src/linux.rs`: 서브프로세스 호출은 모든 monitor 루프 tick에 영향을 준다.
2. **중간** — `maekon-vision/src/trigger.rs` 및 `maekon-monitor/src/input_activity.rs`: lock-poison 처리 (이것들은 저경합 lock이므로 위험이 낮지만 일관성을 위해 패턴을 수정해야 한다).
3. **낮음** — `maekon-storage/src/sqlite/`: 이미 전용 스케줄러 루프 태스크 내에서 실행된다; 순수한 변경을 피하기 위해 미래의 스키마 변경과 함께 `with_conn`으로 마이그레이션한다.

### 코드 리뷰 체크리스트

`crates/` 아래 파일에 대한 풀 리퀘스트 리뷰에 다음 검사를 추가한다.

- [ ] diff가 비동기 함수에서 `std::process::Command`를 도입하는가? 그렇다면 `tokio::process::Command` + `timeout`으로 교체한다.
- [ ] diff가 비동기 함수에서 `std::fs` 함수를 직접 호출하는가? 그렇다면 `tokio::fs` 또는 `spawn_blocking`을 사용한다.
- [ ] diff가 `std::sync` 프리미티브에 대해 `.lock()`, `.read()`, `.write()`를 호출하는가? 결과가 `.expect()`나 `.unwrap()`이 아닌 `.map_err(...)`를 사용하는지 확인한다.
- [ ] 모든 새 `spawn_blocking` 클로저가 `Send + 'static`인가? 빌린 참조가 클로저로 탈출하지 않는지 확인한다.

---

## 관련 ADR

- [ADR-001: Rust Client Architecture Patterns](ADR-001-rust-client-architecture-patterns.md) — 오류 타입 전략(`thiserror` / `anyhow`), 비동기 trait 패턴
- [ADR-002: OS/GUI Interaction Boundary and Runtime Split](ADR-002-os-gui-interaction-boundary.md) — 비동기 런타임 토폴로지; 이 ADR은 해당 토폴로지 내의 블로킹 I/O 경계를 구체화한다
