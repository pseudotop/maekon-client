[English](./oneshim-app.md) | [한국어](./oneshim-app.ko.md)

# oneshim-app

바이너리 진입점. DI 와이어링, 스케줄러, 라이프사이클 관리.

## 역할

- **진입점**: `main()` 함수, 애플리케이션 시작
- **DI 와이어링**: 모든 컴포넌트 조립 및 주입
- **스케줄링**: 주기적 태스크 실행
- **라이프사이클**: 시작/종료 처리
- **자동 업데이트**: GitHub Releases 기반 업데이트

## 디렉토리 구조

> **참고**: `oneshim-app` 패키지는 [ADR-004](../architecture/ADR-004-tauri-v2-migration.ko.md) Tauri v2 마이그레이션에 따라 이제 `src-tauri/` (composition root) 에 위치합니다. 구 `crates/oneshim-app/` 디렉토리는 워크스페이스에서 제거되었습니다.

```
src-tauri/src/  (패키지: oneshim-app)
├── main.rs                      # 진입점 — Tauri builder + DI 와이어링
├── setup.rs, setup_platform.rs, setup_shortcuts.rs, setup_windows.rs
├── lifecycle.rs                 # 시그널 처리, 우아한 종료
├── tray.rs, tray_icon.rs        # 시스템 트레이 메뉴 + 아이콘
├── autostart.rs                 # 자동 시작 설정 (launchd/레지스트리)
├── ipc_error.rs                 # IpcError DTO — Tauri IPC (ADR-019 Follow-up #1)
├── notification_manager.rs      # 쿨다운 기반 알림 매니저
├── commands/                    # 106개 Tauri IPC command 핸들러 (19 파일, 디렉토리 모듈; ADR-019 Follow-up #1 로 모두 `Result<_, IpcError>` 반환)
├── scheduler/                   # 16 루프 백그라운드 스케줄러 (디렉토리 모듈)
│   ├── config.rs, mod.rs
│   ├── analysis_pipeline/, gui_pipeline.rs, heatmap.rs, shared_regime_state.rs
│   └── loops/                   # 16 spawn 함수: monitor, metrics, process, sync,
│                                #   heartbeat, aggregation, notification, focus,
│                                #   event_snapshot, oauth_refresh, analysis,
│                                #   cross_device_sync, coaching + 조건부:
│                                #   health_check, suggestion_sse, suggestion_maintenance
├── updater/                     # 자동 업데이트 (디렉토리 모듈)
│                                # D9 다중키 Ed25519 trust (trusted_keys.rs), D10 방어적
│                                # 롤아웃 처리, D11 self-healthy probe + 자동 rollback
├── focus_analyzer/              # 포커스 분석 (디렉토리 모듈)
├── agent_runtime/, session_manager/, session_adapters/, feedback_sink/
├── provider_adapters/, subprocess_provider/
├── services/                    # 도메인 서비스 (log_helpers 등)
├── telemetry/                   # OpenTelemetry 계측 (디렉토리 모듈)
├── native_border/               # 플랫폼 네이티브 창 테두리 스타일
├── {bootstrap,background,server,web,integration,agent,update}_runtime.rs  # 7 runtime facade
├── app_runtime_launch.rs        # 메인 launch 오케스트레이션 (gRPC UnifiedClient + REST 와이어링)
├── agent_runtime_support.rs     # Agent 모드 runtime 지원
├── magic_overlay.rs, magic_overlay_driver.rs  # ADR-002 M3 WebView 오버레이 브리지
├── update_coordinator.rs, update_runtime.rs
├── platform_overlay.rs, platform_accessibility.rs, macos_integration.rs
├── auditing_session.rs, auth_cli.rs, bridge_cli.rs, secret_cli.rs
├── integrity_guard.rs, integration_policy.rs, integration_insight_source.rs,
│   integration_prompt_delivery.rs
├── capture_services.rs, storage_runtime.rs, sync_engine.rs,
│   fallback_stt.rs, feature_capabilities.rs
├── suggestion_manager.rs, workflow_intelligence.rs
├── bootstrap_preflight.rs
├── desktop_permissions.rs, desktop_startup.rs
├── oauth_provider_registry.rs, provider_secret_backend.rs
├── runtime_bridges.rs, runtime_state.rs, server_runtime_context.rs
├── session_context.rs, focus_auto.rs, focus_mode.rs, focus_probe_adapter.rs
├── skill_loader.rs, log_retention.rs, memory_profiler.rs
├── cli_subscription_bridge.rs   # CLI 구독 브리지 아티팩트 동기화
├── automation_runtime.rs, automation_controller_builder.rs
└── launch_resources.rs
```

## CLI 구독 브리지

`ai_provider.access_mode`가 `ProviderSubscriptionCli`일 때, Maekon 은 외부 AI CLI용 브리지 커맨드 파일을 동기화할 수 있다.

- 자동 설치 활성화: `ONESHIM_CLI_BRIDGE_AUTOINSTALL=1`
- 사용자 스코프 디렉토리(`~/.codex`, `~/.claude`, `~/.gemini`) 포함: `ONESHIM_CLI_BRIDGE_INCLUDE_USER_SCOPE=1`
- 생성되는 브리지 파일의 기본 컨텍스트 경로: `<data_dir>/exports/oneshim-context.json`

참고: 내부 CLI subscription bridge research note

## AI 제공자 어댑터 (`provider_adapters.rs`)

`resolve_ai_provider_adapters()`는 `AiProviderConfig`를 기준으로 OCR/LLM 제공자와 출처 메타데이터를 결정한다.

- 접근 모드:
  - `LocalModel`
  - `ProviderApiKey`
  - `ProviderSubscriptionCli`
- 폴백 동작:
  - `fallback_to_local=true`면 원격 초기화 실패 시 로컬 제공자로 자동 폴백
- OCR 프라이버시 게이트:
  - 원격 OCR 호출은 `GuardedOcrProvider`로 래핑
  - 전송 전 `PrivacyGateway::sanitize_image_for_external_policy()` 적용
  - `allow_unredacted_external_ocr`로 원본 전송 opt-out 허용
  - 응답 후 calibration 검증(`OcrValidationConfig`) 수행

## 주요 컴포넌트

### main.rs

애플리케이션 진입점 및 DI 조립:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. CLI 인자 파싱 + 로깅 초기화 + AppConfig 로드
    // 2. 무결성 preflight 실행, AI 접근 모드 평가
    // 3. 저장소/모니터/네트워크/비전/자동화 컴포넌트 DI 구성
    // 4. 제공자 어댑터 해석 + 필요 시 CLI 브리지 동기화
    // 5. 스케줄러(9-루프) 시작
    // 6. 웹 서버 + 선택적 업데이트 코디네이터 시작
    // 7. 종료 신호 대기 후 세션 종료 기록
    Ok(())
}
```

### Scheduler (`scheduler/`)

현재 스케줄러는 3-루프가 아니라 **9-루프 오케스트레이터**다 (`Scheduler::run()` → `run_scheduler_loops()`).

| 루프 | 주기 | 책임 |
|------|------|------|
| Monitor | 1초 | 컨텍스트 수집, 유휴 전환, 캡처 트리거, 프레임 처리, 이벤트 저장, 선택적 업로드 큐 |
| Metrics | 5초 | 시스템 메트릭 저장 + 대시보드 실시간 브로드캐스트 + 고사용량 알림 체크 |
| Process Snapshot | 10초 | 상위 프로세스 스냅샷 저장 |
| Sync | 10초 | 배치 업로드(플랫폼 연동 모드) + 보존 정책 실행 |
| Heartbeat | 30초 | 연결 모드 서버 하트비트 |
| Aggregation | 1시간 | 시간 집계 + 메트릭/프로세스/유휴 정리 |
| Notification | 1분 | 장시간 작업 알림 점검 |
| Focus | 1분 | `FocusAnalyzer::analyze_periodic()` + 제안 생성 |
| Event Snapshot | 30초 | 상세 프로세스 + `InputActivityCollector` 스냅샷 이벤트 수집 |

핵심 경계:

- 저장소 경계: `SchedulerStorage` 포트(메트릭 저장 + 프레임 메타데이터 저장 확장)
- 업로드/프라이버시 경계: `PlatformEgressPolicy`가 세정/전송 허용 여부 결정
- 프레임 보존: `FrameFileStorage`의 보존/용량 제한을 Sync 루프에서 강제
- 선택 서브시스템: `NotificationManager`, `FocusAnalyzer` 주입 시에만 활성화

### Lifecycle (lifecycle.rs)

시그널 처리 및 graceful shutdown:

```rust
pub struct Lifecycle {
    shutdown_tx: watch::Sender<bool>,
}

impl Lifecycle {
    pub fn new(shutdown_tx: watch::Sender<bool>) -> Self {
        Self { shutdown_tx }
    }

    pub async fn wait_for_shutdown(&self) {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Ctrl+C 핸들러 설치 실패");
        };

        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM 핸들러 설치 실패")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => info!("Ctrl+C 수신"),
            _ = terminate => info!("SIGTERM 수신"),
        }

        info!("종료 시작...");
        let _ = self.shutdown_tx.send(true);
    }
}
```

### EventBus (event_bus.rs)

내부 이벤트 라우팅:

```rust
pub struct EventBus {
    sender: broadcast::Sender<InternalEvent>,
}

#[derive(Clone, Debug)]
pub enum InternalEvent {
    NewSuggestion(Suggestion),
    ConnectionStatusChanged(ConnectionStatus),
    SyncCompleted { events: usize, frames: usize },
    ErrorOccurred(String),
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(100);
        Self { sender }
    }

    pub fn publish(&self, event: InternalEvent) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<InternalEvent> {
        self.sender.subscribe()
    }
}
```

### Autostart (autostart.rs)

로그인 시 자동 시작 설정:

```rust
pub struct Autostart;

impl Autostart {
    #[cfg(target_os = "macos")]
    pub fn enable() -> Result<(), CoreError> {
        let plist = Self::generate_launchagent_plist()?;
        let path = dirs::home_dir()
            .unwrap()
            .join("Library/LaunchAgents/com.oneshim.client.plist");
        std::fs::write(&path, plist)?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub fn disable() -> Result<(), CoreError> {
        let path = dirs::home_dir()
            .unwrap()
            .join("Library/LaunchAgents/com.oneshim.client.plist");
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    pub fn enable() -> Result<(), CoreError> {
        use windows_sys::Win32::System::Registry::*;

        let exe_path = std::env::current_exe()?;
        let key_path = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            RegOpenKeyExW(HKEY_CURRENT_USER, /* ... */)?;
            RegSetValueExW(hkey, "ONESHIM", /* exe_path */)?;
            RegCloseKey(hkey);
        }
        Ok(())
    }
}
```

### Updater (`updater/`)

자동 업데이트는 `Updater` + `update_coordinator` 조합으로 동작한다.

- 기준 API: `https://api.github.com/repos/{owner}/{repo}/releases/latest`
- 버전 정책:
  - `CURRENT_VERSION` 대비 semver 비교
  - `include_prerelease` 기반 프리릴리스 필터링
  - 최소 허용 버전 floor 검사
- 에셋 정책:
  - OS/arch 패턴 기반 플랫폼별 에셋 선택
  - 다운로드 전 허용 호스트 allowlist 검증
- 무결성 정책:
  - `.sha256` 기반 SHA-256 검증
  - 선택적 Ed25519 서명 강제(`require_signature_verification=true`, `update.signature_public_key`)
- 설치:
  - 경로 이탈(path traversal) 방지 압축 해제
  - `self_update::self_replace` 기반 바이너리 교체

### 인스톨러/릴리스 패키징 연계

릴리스 아티팩트는 `.github/workflows/release.yml`에서 생성되며, 다음 경로에서 공통으로 소비한다.

- 앱 내 업데이트: `src-tauri/src/updater/`
- 터미널 인스톨러:
  - `scripts/install.sh`
  - `scripts/install.ps1`
  - `scripts/uninstall.sh`
  - `scripts/uninstall.ps1`

인스톨러와 업데이터는 동일한 릴리스 에셋/체크섬/서명 사이드카를 사용하므로 설치와 업데이트 경로의 무결성 정책이 일치한다.

## 실행 흐름

1. `main.rs` / `gui_runner.rs`에서 설정 로드 후 DI 구성(저장소, 모니터, 네트워크, 비전, 자동화, 웹).
2. 접근 모드(`LocalModel`, `ProviderApiKey`, `ProviderSubscriptionCli`, `ProviderOAuth`)를 평가하고 AI 어댑터 해석.
3. 구독 모드에서는 CLI 브리지 아티팩트를 선택적으로 동기화.
4. 스케줄러 9개 루프와 선택 서브시스템(알림/집중도/실시간 이벤트) 기동.
5. 웹 서버가 API + 임베디드 프론트엔드를 제공하고 실시간 이벤트를 소비.
6. 업데이트 코디네이터가 릴리스 채널을 점검하고 설치 액션을 처리.
7. 종료 신호 수신 시 세션 종료 기록 후 graceful shutdown.

## 의존성

- `anyhow`: 바이너리 에러 처리
- `tokio`: 비동기 런타임
- `tracing-subscriber`: 로깅
- `config`: 설정 파일 파싱
- `directories`: 플랫폼별 디렉토리
- `self_update`: 바이너리 업데이트
- `semver`: 버전 비교

## 빌드

```bash
# 개발 빌드
cargo build -p oneshim-app

# 릴리즈 빌드
cargo build --release -p oneshim-app

# 실행
cargo run -p oneshim-app
```

## 환경 변수

| 변수 | 필수 | 설명 |
|------|------|------|
| `ONESHIM_EMAIL` | 연결 모드에서만 ✅ | 로그인 이메일 (standalone 모드에서는 선택) |
| `ONESHIM_PASSWORD` | 연결 모드에서만 ✅ | 로그인 비밀번호 (standalone 모드에서는 선택) |
| `RUST_LOG` | ❌ | 로그 레벨 (기본: `info`) |
| `ONESHIM_CONFIG` | ❌ | 설정 파일 경로 |

## 테스트

```rust
#[test]
fn updater_rejects_unknown_download_host() {
    let updater = Updater::new(test_config());
    let result = updater.validate_download_url("https://evil.example.com/file.tar.gz");
    assert!(result.is_err());
}

#[test]
fn verify_signature_accepts_valid_ed25519_signature() {
    // updater.rs의 실제 테스트에서 서명 검증 경로를 커버한다.
}
```
