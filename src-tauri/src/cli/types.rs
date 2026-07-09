//! Debug CLI command type definitions (debug_assertions only).
//!
//! All types are `pub(crate)` and re-exported from `cli/mod.rs`.

#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebugAutostartCliCommand {
    Status,
    Enable,
    Disable,
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebugPermissionsCliCommand {
    Status,
    ScreenCaptureRequest,
    ScreenCaptureAttempt,
    AccessibilityRequest,
    OpenAccessibilitySettings,
    OpenScreenCaptureSettings,
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DebugAxTreeCliCommand {
    Extract {
        app_name: String,
        max_depth: u32,
        max_elements: usize,
    },
    WindowsUiaBenchmark {
        samples: u32,
        max_depth: u32,
        max_elements: usize,
    },
    WindowsOcrBenchmark {
        samples: u32,
        display_scale_x100: u32,
    },
    WindowsGuiSessionE2eBenchmark {
        display_scale_x100: u32,
        overlay_hold_ms: u64,
    },
    WindowsSandboxOverheadBenchmark {
        samples: u32,
        timeout_probe_ms: u64,
    },
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebugNotificationCliCommand {
    Status,
    Request,
    Send,
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebugPointerCaptureCliCommand {
    Probe { frames: u32, interval_ms: u64 },
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebugPointerCaptureRuntimeCliCommand {
    OverlayProbe { frames: u32, interval_ms: u64 },
    OverlayProbeReducedMotion { frames: u32, interval_ms: u64 },
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebugNotificationBackend {
    TauriPlugin,
    MacosUnuser,
}

#[cfg(debug_assertions)]
impl DebugNotificationBackend {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::TauriPlugin => "tauri-plugin",
            Self::MacosUnuser => "macos-unuser",
        }
    }
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebugPermissionsRuntimeCliCommand {
    ScreenCaptureRequest,
}
