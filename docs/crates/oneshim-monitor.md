[English](./oneshim-monitor.md) | [한국어](./oneshim-monitor.ko.md)

# oneshim-monitor

The crate for system and user activity monitoring.

## Role

- **System Metrics**: CPU, memory, disk, and network usage
- **Process Tracking**: Active windows, running processes
- **Activity Detection**: Keyboard/mouse activity, idle state

## Directory Structure

```
oneshim-monitor/src/
├── lib.rs              # Crate root
├── system.rs           # SysInfoMonitor - CPU/Memory/Disk/Network (sysinfo 0.38)
├── system_info.rs      # System information wrappers
├── process.rs          # ProcessTracker - active process/window tracking
├── activity.rs         # ActivityTracker - idle detection
├── idle.rs             # Cross-platform idle detection helpers
├── input_activity.rs   # InputActivityCollector - mouse/keyboard pattern (atomic counters)
├── input_detail.rs     # Richer input event details (superpowers-era)
├── keyboard_pattern.rs # Keyboard pattern matching
├── key_hook/           # Low-level key-hook directory module (platform-branched)
├── clipboard.rs        # Clipboard-change tracking
├── file_access.rs      # File-access telemetry
├── window_layout.rs    # WindowLayoutTracker - window layout changes
├── macos.rs            # macOS-specific (osascript + circuit_breaker_skips_when_tripped with serial_test)
├── windows.rs          # Windows-specific (Win32 GetForegroundWindow + sysinfo)
├── linux.rs            # Linux-specific (xdotool/xprintidle X11 + Wayland XWayland fallback)
└── error.rs            # MonitorError (ADR-019 typed codes)
```

## Key Components

### SysInfoMonitor (system.rs)

System metrics collection based on `sysinfo` (`SystemMonitor` port):

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

**Collected Metrics**:
| Category | Metrics |
|----------|---------|
| CPU | Usage (%), core count |
| Memory | Total/used/available bytes |
| Disk | Total/used bytes, read/write speed |
| Network | Received/sent bytes, packet count |

### ProcessTracker (process.rs)

Active process and window tracking (`ProcessMonitor` port):

```rust
pub struct ProcessTracker {
    sys: RwLock<System>,
}

impl ProcessMonitor for ProcessTracker {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError>;
    async fn get_running_processes(&self) -> Result<Vec<ProcessInfo>, CoreError>;
}
```

**WindowInfo Structure**:
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

User activity detection (`ActivityMonitor` port):

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

## Platform-Specific Implementations

### macOS (macos.rs)

Active window detection based on `osascript`:

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

    // Parse and create WindowInfo
}
```

### Windows (windows.rs)

Active window detection based on Win32 API:

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

        // Get window title
        let mut title = [0u16; 256];
        GetWindowTextW(hwnd, title.as_mut_ptr(), 256);

        // Get PID
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);

        // Look up process name via sysinfo
        // ...
    }
}
```

## Monitoring Loop

Called from the scheduler in `oneshim-app`:

```
┌─────────────────────────────────────────────────────────┐
│                    Scheduler (1-second interval)         │
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

## Event Generation

Converts monitoring results to `ContextEvent`:

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

## Dependencies

- `sysinfo`: System information (0.32)
- `windows-sys`: Windows API (Windows only)

## Platform Support

| Platform | System Metrics | Active Window | Process List |
|----------|---------------|---------------|--------------|
| macOS | ✅ | ✅ (osascript) | ✅ |
| Windows | ✅ | ✅ (Win32) | ✅ |
| Linux | ✅ | ⚠️ (X11 only) | ✅ |

## Tests

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
    let tracker = ActivityTracker::new(5); // 5-second threshold

    // Record activity
    tracker.record_activity().await.unwrap();
    assert!(!tracker.is_idle().await.unwrap());

    // After time elapses (simulated in tests)
    // assert!(tracker.is_idle().await.unwrap());
}
```

## Performance Considerations

1. **Polling interval**: 1 second default, configurable
2. **sysinfo caching**: Minimize `refresh_all()` calls
3. **Platform optimization**: Use native APIs for each OS
