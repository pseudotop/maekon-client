[English](./oneshim-monitor.md) | [한국어](./oneshim-monitor.ko.md)

# oneshim-monitor

시스템 및 사용자 활동 모니터링 크레이트.

## 역할

- **시스템 메트릭**: CPU, 메모리, 디스크, 네트워크 사용량
- **프로세스 추적**: 활성 창, 실행 중인 프로세스
- **활동 감지**: 키보드/마우스 활동, 유휴 상태

## 디렉토리 구조

```
oneshim-monitor/src/
├── lib.rs              # 크레이트 루트
├── system.rs           # SysInfoMonitor - CPU/메모리/디스크/네트워크 (sysinfo 0.38)
├── system_info.rs      # 시스템 정보 래퍼
├── process.rs          # ProcessTracker - 활성 프로세스/창 추적
├── activity.rs         # ActivityTracker - 유휴 감지
├── idle.rs             # 크로스 플랫폼 유휴 감지 헬퍼
├── input_activity.rs   # InputActivityCollector - 마우스/키보드 패턴 (atomic counter)
├── input_detail.rs     # 세밀한 입력 이벤트 (superpowers-era)
├── keyboard_pattern.rs # 키보드 패턴 매칭
├── key_hook/           # 저수준 key-hook 디렉토리 모듈 (플랫폼 분기)
├── clipboard.rs        # 클립보드 변경 추적
├── file_access.rs      # 파일 접근 텔레메트리
├── window_layout.rs    # WindowLayoutTracker - 창 레이아웃 변경
├── macos.rs            # macOS 전용 (osascript + serial_test로 circuit_breaker_skips_when_tripped 직렬화)
├── windows.rs          # Windows 전용 (Win32 GetForegroundWindow + sysinfo)
├── linux.rs            # Linux 전용 (xdotool/xprintidle X11 + Wayland XWayland 폴백)
└── error.rs            # MonitorError (ADR-019 typed code)
```

## 주요 컴포넌트

### SysInfoMonitor (system.rs)

`sysinfo` 기반 시스템 메트릭 수집 (`SystemMonitor` 포트):

```rust
pub struct SysInfoMonitor {
    sys: RwLock<System>,
}

impl SystemMonitor for SysInfoMonitor {
    async fn get_metrics(&self) -> Result<SystemMetrics, CoreError> {
        let mut sys = self.sys.write().await;
        sys.refresh_all();

        Ok(SystemMetrics {
            cpu: CpuMetrics {
                usage_percent: sys.global_cpu_usage(),
                core_count: sys.cpus().len() as u32,
            },
            memory: MemoryMetrics {
                total_bytes: sys.total_memory(),
                used_bytes: sys.used_memory(),
                available_bytes: sys.available_memory(),
            },
            disk: DiskMetrics { /* ... */ },
            network: NetworkMetrics { /* ... */ },
        })
    }
}
```

**수집 메트릭**:
| 카테고리 | 메트릭 |
|----------|--------|
| CPU | 사용률(%), 코어 수 |
| 메모리 | 전체/사용/가용 바이트 |
| 디스크 | 전체/사용 바이트, 읽기/쓰기 속도 |
| 네트워크 | 수신/송신 바이트, 패킷 수 |

### ProcessTracker (process.rs)

활성 프로세스 및 창 추적 (`ProcessMonitor` 포트):

```rust
pub struct ProcessTracker {
    sys: RwLock<System>,
}

impl ProcessMonitor for ProcessTracker {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError>;
    async fn get_running_processes(&self) -> Result<Vec<ProcessInfo>, CoreError>;
}
```

**WindowInfo 구조**:
```rust
pub struct WindowInfo {
    pub window_id: u64,
    pub title: String,
    pub app_name: String,
    pub pid: u32,
    pub is_focused: bool,
}
```

### ActivityTracker (activity.rs)

사용자 활동 감지 (`ActivityMonitor` 포트):

```rust
pub struct ActivityTracker {
    idle_threshold_secs: u64,
    last_activity: RwLock<Instant>,
}

impl ActivityMonitor for ActivityTracker {
    async fn is_idle(&self) -> Result<bool, CoreError> {
        let last = self.last_activity.read().await;
        Ok(last.elapsed().as_secs() > self.idle_threshold_secs)
    }

    async fn record_activity(&self) -> Result<(), CoreError> {
        *self.last_activity.write().await = Instant::now();
        Ok(())
    }
}
```

## 플랫폼별 구현

### macOS (macos.rs)

`osascript` 기반 활성 창 감지:

```rust
#[cfg(target_os = "macos")]
pub fn get_active_window_macos() -> Result<Option<WindowInfo>, CoreError> {
    let script = r#"
        tell application "System Events"
            set frontApp to first application process whose frontmost is true
            set appName to name of frontApp
            tell frontApp
                set windowTitle to name of front window
            end tell
        end tell
        return appName & "|" & windowTitle
    "#;

    let output = Command::new("osascript")
        .args(["-e", script])
        .output()?;

    // 파싱 및 WindowInfo 생성
}
```

### Windows (windows.rs)

Win32 API 기반 활성 창 감지:

```rust
#[cfg(target_os = "windows")]
pub fn get_active_window_windows() -> Result<Option<WindowInfo>, CoreError> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    use windows_sys::Win32::Foundation::*;

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return Ok(None);
        }

        // 창 제목 가져오기
        let mut title = [0u16; 256];
        GetWindowTextW(hwnd, title.as_mut_ptr(), 256);

        // PID 가져오기
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);

        // sysinfo로 프로세스 이름 조회
        // ...
    }
}
```

## 모니터링 루프

`oneshim-app`의 스케줄러에서 호출:

```
┌─────────────────────────────────────────────────────────┐
│                    Scheduler (1초 간격)                   │
└─────────────────────────────────────────────────────────┘
        │                    │                    │
        ▼                    ▼                    ▼
┌───────────────┐   ┌───────────────┐   ┌───────────────┐
│ SystemMonitor │   │ ProcessMonitor│   │ ActivityMonitor│
│  get_metrics  │   │get_active_window│  │   is_idle     │
└───────────────┘   └───────────────┘   └───────────────┘
        │                    │                    │
        └────────────────────┼────────────────────┘
                             ▼
                     ContextPayload
                             │
                             ▼
                      BatchUploader
```

## 이벤트 생성

모니터링 결과를 `ContextEvent`로 변환:

```rust
pub fn create_context_event(
    window: Option<WindowInfo>,
    metrics: SystemMetrics,
    is_idle: bool,
) -> ContextEvent {
    let event_type = if is_idle {
        EventType::Idle
    } else if let Some(w) = &window {
        if w.is_focused {
            EventType::WindowFocus
        } else {
            EventType::ApplicationSwitch
        }
    } else {
        EventType::Unknown
    };

    ContextEvent {
        event_id: Uuid::new_v4().to_string(),
        event_type,
        window_title: window.map(|w| w.title),
        app_name: window.map(|w| w.app_name),
        timestamp: Utc::now(),
        metadata: serde_json::json!({
            "cpu_usage": metrics.cpu.usage_percent,
            "memory_usage": metrics.memory.used_bytes as f64 / metrics.memory.total_bytes as f64,
        }),
    }
}
```

## 의존성

- `sysinfo`: 시스템 정보 (0.32)
- `windows-sys`: Windows API (Windows only)

## 플랫폼 지원

| 플랫폼 | 시스템 메트릭 | 활성 창 | 프로세스 목록 |
|--------|-------------|---------|--------------|
| macOS | ✅ | ✅ (osascript) | ✅ |
| Windows | ✅ | ✅ (Win32) | ✅ |
| Linux | ✅ | ⚠️ (X11 only) | ✅ |

## 테스트

```rust
#[tokio::test]
async fn test_system_metrics() {
    let monitor = SysInfoMonitor::new();
    let metrics = monitor.get_metrics().await.unwrap();

    assert!(metrics.cpu.usage_percent >= 0.0);
    assert!(metrics.cpu.usage_percent <= 100.0);
    assert!(metrics.memory.total_bytes > 0);
}

#[tokio::test]
async fn test_idle_detection() {
    let tracker = ActivityTracker::new(5); // 5초 임계값

    // 활동 기록
    tracker.record_activity().await.unwrap();
    assert!(!tracker.is_idle().await.unwrap());

    // 시간 경과 후 (테스트에서는 시뮬레이션)
    // assert!(tracker.is_idle().await.unwrap());
}
```

## 성능 고려사항

1. **폴링 간격**: 1초 기본값, 설정 가능
2. **sysinfo 캐싱**: `refresh_all()` 호출 최소화
3. **플랫폼 최적화**: 각 OS의 네이티브 API 활용
