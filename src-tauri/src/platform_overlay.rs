use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;
use uuid::Uuid;

use oneshim_core::error::CoreError;
use oneshim_core::models::gui::{HighlightHandle, HighlightRequest};
use oneshim_core::models::ui_scene::UiScene;
use oneshim_core::ports::overlay_driver::OverlayDriver;

#[derive(Debug)]
struct OverlayProcess {
    child: Child,
    payload_path: PathBuf,
}

pub fn create_platform_overlay_driver() -> Arc<dyn OverlayDriver> {
    Arc::new(PlatformOverlayDriver::new())
}

pub struct PlatformOverlayDriver {
    active_processes: Mutex<HashMap<String, OverlayProcess>>,
}

impl PlatformOverlayDriver {
    pub fn new() -> Self {
        Self {
            active_processes: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl OverlayDriver for PlatformOverlayDriver {
    async fn show_highlights(&self, req: HighlightRequest) -> Result<HighlightHandle, CoreError> {
        if req.targets.is_empty() {
            return Err(CoreError::InvalidArguments {
                code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
                message: "Overlay request requires at least one highlight target".to_string(),
            });
        }

        let handle_id = Uuid::new_v4().to_string();
        let payload_path = write_overlay_payload(&handle_id, &req)?;
        let child = spawn_overlay_process(&payload_path)?;

        {
            let mut active = self.active_processes.lock().await;
            active.insert(
                handle_id.clone(),
                OverlayProcess {
                    child,
                    payload_path,
                },
            );
        }

        Ok(HighlightHandle {
            handle_id,
            rendered_at: Utc::now(),
            target_count: req.targets.len(),
        })
    }

    async fn clear_highlights(&self, handle_id: &str) -> Result<(), CoreError> {
        let mut active = self.active_processes.lock().await;
        let Some(mut process) = active.remove(handle_id) else {
            return Ok(());
        };

        if let Err(e) = process.child.kill() {
            debug!("process kill failed: {e}");
        }
        if let Err(e) = process.child.wait() {
            debug!("process wait failed: {e}");
        }
        if let Err(e) = fs::remove_file(process.payload_path) {
            debug!("remove_file failed: {e}");
        }

        Ok(())
    }

    async fn show_detection(&self, scene: &UiScene) -> Result<(), CoreError> {
        tracing::debug!(
            scene_id = %scene.scene_id,
            element_count = scene.elements.len(),
            "PlatformOverlayDriver: detection not supported on platform overlay"
        );
        Ok(())
    }

    async fn clear_detection(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct OverlayPayload<'a> {
    session_id: &'a str,
    scene_id: &'a str,
    targets: Vec<OverlayTarget<'a>>,
}

#[derive(Debug, Serialize)]
struct OverlayTarget<'a> {
    candidate_id: &'a str,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: &'a str,
}

fn write_overlay_payload(handle_id: &str, req: &HighlightRequest) -> Result<PathBuf, CoreError> {
    let payload = OverlayPayload {
        session_id: &req.session_id,
        scene_id: &req.scene_id,
        targets: req
            .targets
            .iter()
            .map(|target| OverlayTarget {
                candidate_id: &target.candidate_id,
                x: target.bbox_abs.x,
                y: target.bbox_abs.y,
                width: target.bbox_abs.width,
                height: target.bbox_abs.height,
                color: &target.color,
            })
            .collect(),
    };

    let path = std::env::temp_dir().join(format!("oneshim-overlay-{handle_id}.json"));
    let bytes = serde_json::to_vec(&payload).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("Overlay payload serialization failed: {e}"),
    })?;
    // Iter-95: was rebuilding io::Error via io::Error::new(kind, to_string()),
    // which flattens the original error's source chain. Use `?` with the
    // CoreError::Io #[from] conversion to preserve the original std::io::Error.
    fs::write(&path, bytes)?;

    Ok(path)
}

fn spawn_overlay_process(payload_path: &PathBuf) -> Result<Child, CoreError> {
    #[cfg(target_os = "windows")]
    {
        spawn_windows_overlay(payload_path)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        spawn_python_overlay(payload_path)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = payload_path;
        Err(CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: "Overlay driver is not available on this platform".to_string(),
        })
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn spawn_python_overlay(payload_path: &PathBuf) -> Result<Child, CoreError> {
    const PYTHON_OVERLAY_SCRIPT: &str = r#"
import json
import sys
import tkinter as tk

with open(sys.argv[1], 'r', encoding='utf-8') as f:
    payload = json.load(f)

root = tk.Tk()
root.withdraw()

thickness = 3
windows = []

for target in payload.get('targets', []):
    x = int(target.get('x', 0))
    y = int(target.get('y', 0))
    w = max(1, int(target.get('width', 1)))
    h = max(1, int(target.get('height', 1)))
    color = target.get('color', '#22c55e')

    rects = [
        (x, y, w, thickness),
        (x, y + h - thickness, w, thickness),
        (x, y, thickness, h),
        (x + w - thickness, y, thickness, h),
    ]

    for rx, ry, rw, rh in rects:
        overlay = tk.Toplevel(root)
        overlay.overrideredirect(True)
        try:
            overlay.attributes('-topmost', True)
        except Exception:
            pass
        overlay.geometry(f'{max(1, rw)}x{max(1, rh)}+{rx}+{ry}')
        canvas = tk.Canvas(overlay, highlightthickness=0, bg=color)
        canvas.pack(fill='both', expand=True)
        windows.append(overlay)

root.mainloop()
"#;

    let mut command = Command::new("python3");
    command
        .arg("-c")
        .arg(PYTHON_OVERLAY_SCRIPT)
        .arg(payload_path);

    match command.spawn() {
        Ok(child) => Ok(child),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut fallback = Command::new("python");
            fallback
                .arg("-c")
                .arg(PYTHON_OVERLAY_SCRIPT)
                .arg(payload_path);
            fallback.spawn().map_err(|e| CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: format!("Python overlay runtime unavailable (python3/python): {e}"),
            })
        }
        Err(err) => Err(CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: format!("Failed to launch Python overlay process: {err}"),
        }),
    }
}

#[cfg(target_os = "windows")]
fn spawn_windows_overlay(payload_path: &PathBuf) -> Result<Child, CoreError> {
    const POWERSHELL_OVERLAY_SCRIPT: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$json = Get-Content -Raw -Path $args[0] | ConvertFrom-Json
$forms = @()
$thickness = 3

foreach ($target in $json.targets) {
    $x = [int]$target.x
    $y = [int]$target.y
    $w = [Math]::Max(1, [int]$target.width)
    $h = [Math]::Max(1, [int]$target.height)
    $color = [System.Drawing.ColorTranslator]::FromHtml($target.color)

    $rects = @(
        @{ X=$x; Y=$y; W=$w; H=$thickness },
        @{ X=$x; Y=($y + $h - $thickness); W=$w; H=$thickness },
        @{ X=$x; Y=$y; W=$thickness; H=$h },
        @{ X=($x + $w - $thickness); Y=$y; W=$thickness; H=$h }
    )

    foreach ($rect in $rects) {
        $form = New-Object System.Windows.Forms.Form
        $form.FormBorderStyle = 'None'
        $form.StartPosition = 'Manual'
        $form.ShowInTaskbar = $false
        $form.TopMost = $true
        $form.BackColor = $color
        $form.Opacity = 0.70
        $form.Location = New-Object System.Drawing.Point($rect.X, $rect.Y)
        $form.Size = New-Object System.Drawing.Size([Math]::Max(1,$rect.W), [Math]::Max(1,$rect.H))
        $forms += $form
    }
}

foreach ($form in $forms) {
    $null = $form.Show()
}

[System.Windows.Forms.Application]::Run()
"#;

    Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            POWERSHELL_OVERLAY_SCRIPT,
        ])
        .arg(payload_path)
        .spawn()
        .map_err(|e| CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: format!("Failed to launch Windows overlay process: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::gui::HighlightTarget;
    use oneshim_core::models::intent::ElementBounds;

    fn test_target(id: &str) -> HighlightTarget {
        HighlightTarget {
            candidate_id: id.to_string(),
            bbox_abs: ElementBounds {
                x: 10,
                y: 20,
                width: 100,
                height: 30,
            },
            color: "#22c55e".to_string(),
            label: Some("Save".to_string()),
        }
    }

    fn test_request(targets: Vec<HighlightTarget>) -> HighlightRequest {
        HighlightRequest {
            session_id: "s1".to_string(),
            scene_id: "scene".to_string(),
            targets,
        }
    }

    #[test]
    fn payload_file_is_written() {
        let req = test_request(vec![test_target("el-1")]);
        let path = write_overlay_payload("unit-test-write", &req).unwrap();
        assert!(path.exists());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn payload_contains_correct_json_structure() {
        let req = test_request(vec![test_target("el-1")]);
        let path = write_overlay_payload("unit-test-json", &req).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(parsed["session_id"], "s1");
        assert_eq!(parsed["scene_id"], "scene");
        assert_eq!(parsed["targets"][0]["candidate_id"], "el-1");
        assert_eq!(parsed["targets"][0]["x"], 10);
        assert_eq!(parsed["targets"][0]["y"], 20);
        assert_eq!(parsed["targets"][0]["width"], 100);
        assert_eq!(parsed["targets"][0]["height"], 30);
        assert_eq!(parsed["targets"][0]["color"], "#22c55e");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn payload_with_multiple_targets() {
        let req = test_request(vec![test_target("el-1"), test_target("el-2")]);
        let path = write_overlay_payload("unit-test-multi", &req).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(parsed["targets"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["targets"][1]["candidate_id"], "el-2");

        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn show_highlights_rejects_empty_targets() {
        let driver = PlatformOverlayDriver::new();
        let req = test_request(vec![]);
        let err = driver.show_highlights(req).await.unwrap_err();
        assert!(matches!(err, CoreError::InvalidArguments { .. }));
    }

    #[tokio::test]
    async fn clear_highlights_ignores_unknown_handle() {
        let driver = PlatformOverlayDriver::new();
        // Should not error even when handle does not exist
        let result = driver.clear_highlights("nonexistent-handle").await;
        assert!(result.is_ok());
    }
}
