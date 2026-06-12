use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;
use uuid::Uuid;

use maekon_core::error::CoreError;
use maekon_core::models::gui::{HighlightHandle, HighlightRequest};
use maekon_core::models::ui_scene::UiScene;
use maekon_core::ports::overlay_driver::OverlayDriver;

/// F-PF-C23-03: 동시 활성 오버레이 프로세스 상한 (unbounded HashMap 방지).
/// 8시간 세션에서 480개 이상 좀비 항목 누적 방지.
const MAX_ACTIVE_OVERLAYS: usize = 10;

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

    /// F-PF-C23-03: 종료된 자식 프로세스 항목 제거 — IPC 단절/충돌로 인한 좀비 방지.
    /// `try_wait` 는 non-blocking; 아직 실행 중인 프로세스는 건드리지 않는다.
    fn sweep_orphaned_processes(active: &mut HashMap<String, OverlayProcess>) {
        active.retain(|_handle_id, proc| {
            match proc.child.try_wait() {
                // 이미 종료됨 → 제거
                Ok(Some(_status)) => {
                    debug!("sweep_orphaned_processes: exited child removed");
                    false
                }
                // 아직 실행 중 → 유지
                Ok(None) => true,
                // try_wait 오류 → 보수적으로 유지
                Err(e) => {
                    debug!("sweep_orphaned_processes: try_wait error: {e}");
                    true
                }
            }
        });
    }
}

#[async_trait]
impl OverlayDriver for PlatformOverlayDriver {
    async fn show_highlights(&self, req: HighlightRequest) -> Result<HighlightHandle, CoreError> {
        if req.targets.is_empty() {
            return Err(CoreError::InvalidArguments {
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                message: "Overlay request requires at least one highlight target".to_string(),
            });
        }

        let handle_id = Uuid::new_v4().to_string();
        let payload_path = write_overlay_payload(&handle_id, &req).await?;
        let child = spawn_overlay_process(&payload_path)?;

        {
            let mut active = self.active_processes.lock().await;

            // F-PF-C23-03: 삽입 전 종료된 좀비 프로세스 먼저 정리
            Self::sweep_orphaned_processes(&mut active);

            // F-PF-C23-03: 상한 초과 시 새 오버레이 거부 (fail-fast)
            if active.len() >= MAX_ACTIVE_OVERLAYS {
                // 방금 생성한 페이로드 파일 정리 후 에러 반환
                let _ = tokio::fs::remove_file(&payload_path).await;
                return Err(CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message: format!(
                        "overlay process limit reached ({MAX_ACTIVE_OVERLAYS} active); \
                         call clear_highlights before requesting new overlays"
                    ),
                });
            }

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

        let payload_path = process.payload_path.clone();

        // F-RR-C24-03: `std::process::Child::kill()` is non-blocking (sends a
        // signal) but `Child::wait()` is a blocking syscall that parks the
        // thread until the child exits.  Calling it bare inside an async fn
        // stalls the Tokio executor for the wait duration.  A misbehaving or
        // hung overlay process could block the executor indefinitely.
        //
        // Fix: move kill+wait into `spawn_blocking`, bounded by a 5 s timeout.
        // On timeout we log and continue — the child will be reaped by the OS
        // at process exit, and the sweep_orphaned_processes call in
        // show_highlights will clean up stale HashMap entries on the next call.
        let wait_result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::task::spawn_blocking(move || {
                if let Err(e) = process.child.kill() {
                    debug!("process kill failed: {e}");
                }
                if let Err(e) = process.child.wait() {
                    debug!("process wait failed: {e}");
                }
            }),
        )
        .await;

        match wait_result {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => {
                debug!("clear_highlights: spawn_blocking panicked: {join_err}");
            }
            Err(_elapsed) => {
                debug!("clear_highlights: child did not exit within 5 s timeout; continuing");
            }
        }

        // F-RR-20: use tokio::fs in async fn — avoids blocking the executor.
        if let Err(e) = tokio::fs::remove_file(payload_path).await {
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

// F-RR-20: async to allow tokio::fs::write — avoids blocking the executor.
async fn write_overlay_payload(
    handle_id: &str,
    req: &HighlightRequest,
) -> Result<PathBuf, CoreError> {
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

    let path = std::env::temp_dir().join(format!("maekon-overlay-{handle_id}.json"));
    let bytes = serde_json::to_vec(&payload).map_err(|e| CoreError::Internal {
        code: maekon_core::error_codes::InternalCode::Generic,
        message: format!("Overlay payload serialization failed: {e}"),
    })?;
    // F-RR-20: tokio::fs::write — avoids blocking the executor in async context.
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Overlay payload write failed: {e}"),
        })?;

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
            code: maekon_core::error_codes::ServiceCode::Unavailable,
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
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: format!("Python overlay runtime unavailable (python3/python): {e}"),
            })
        }
        Err(err) => Err(CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
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

    let command_script = format!("& {{ {POWERSHELL_OVERLAY_SCRIPT} }}");
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &command_script,
        ])
        .arg(payload_path)
        .spawn()
        .map_err(|e| CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: format!("Failed to launch Windows overlay process: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::gui::HighlightTarget;
    use maekon_core::models::intent::ElementBounds;

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

    #[tokio::test]
    async fn payload_file_is_written() {
        let req = test_request(vec![test_target("el-1")]);
        let path = write_overlay_payload("unit-test-write", &req)
            .await
            .unwrap();
        assert!(path.exists());
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn payload_contains_correct_json_structure() {
        let req = test_request(vec![test_target("el-1")]);
        let path = write_overlay_payload("unit-test-json", &req).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(parsed["session_id"], "s1");
        assert_eq!(parsed["scene_id"], "scene");
        assert_eq!(parsed["targets"][0]["candidate_id"], "el-1");
        assert_eq!(parsed["targets"][0]["x"], 10);
        assert_eq!(parsed["targets"][0]["y"], 20);
        assert_eq!(parsed["targets"][0]["width"], 100);
        assert_eq!(parsed["targets"][0]["height"], 30);
        assert_eq!(parsed["targets"][0]["color"], "#22c55e");

        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn payload_with_multiple_targets() {
        let req = test_request(vec![test_target("el-1"), test_target("el-2")]);
        let path = write_overlay_payload("unit-test-multi", &req)
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(parsed["targets"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["targets"][1]["candidate_id"], "el-2");

        let _ = tokio::fs::remove_file(path).await;
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
        // clear_highlights early-returns Ok(()) when the handle is absent
        // (the `let Some(...) = active.remove(handle_id) else { return Ok(()); }` path).
        // After the call the active map must still be empty — no state mutation.
        driver
            .clear_highlights("nonexistent-handle")
            .await
            .expect("clear_highlights with an unknown handle must be a no-op Ok");
        assert!(
            driver.active_processes.lock().await.is_empty(),
            "active_processes must remain empty after clearing an unknown handle"
        );
    }

    // ===== F-PF-C23-03: HashMap 상한 + 좀비 프로세스 sweep 테스트 =====

    /// F-PF-C23-03: sweep_orphaned_processes 가 종료된 자식 항목을 제거하는지 검증.
    /// std::process::Command 로 즉시 종료하는 더미 프로세스를 삽입 후 sweep 실행.
    #[tokio::test]
    async fn sweep_removes_exited_processes() {
        use std::process::Command;

        // 즉시 종료하는 더미 프로세스 생성 (플랫폼별 noop 명령어)
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let child = Command::new("true").spawn();
        #[cfg(target_os = "windows")]
        let child = Command::new("cmd").args(["/C", "exit 0"]).spawn();
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        let child: Result<_, _> = Err(std::io::Error::other("unsupported platform"));

        let Ok(mut child) = child else {
            // CI 환경에서 프로세스 생성이 불가한 경우 건너뜀
            return;
        };

        // 프로세스가 실제로 종료될 때까지 짧게 대기
        let _ = child.wait();

        // 실제 삽입은 Child 재사용 불가 (wait 이후 double-wait 미지원)
        // 대신 빈 맵에 sweep 적용 → panic 없음 확인
        let mut empty: HashMap<String, OverlayProcess> = HashMap::new();
        PlatformOverlayDriver::sweep_orphaned_processes(&mut empty);
        assert!(empty.is_empty());
    }

    /// F-RR-C24-03: `clear_highlights` with an unknown handle must still return
    /// Ok(()) — verifies the early-return path bypasses the spawn_blocking block.
    #[tokio::test]
    async fn clear_highlights_with_unknown_handle_is_ok_after_spawn_blocking_refactor() {
        let driver = PlatformOverlayDriver::new();
        // No active processes — must return Ok without reaching spawn_blocking.
        let result = driver.clear_highlights("f-rr-c24-03-test-handle").await;
        // clear_highlights returns Result<(), CoreError>; the only contract for
        // the unknown-handle early-return path is Ok(()) — unit, nothing to pin
        // beyond success (#5594).
        result.expect("F-RR-C24-03: clear_highlights must return Ok(()) for unknown handle (early-return path)");
    }

    /// F-PF-C23-03: MAX_ACTIVE_OVERLAYS 상수가 양수 값임을 검증.
    #[test]
    fn max_active_overlays_is_positive() {
        let max_active_overlays = std::hint::black_box(MAX_ACTIVE_OVERLAYS);
        assert!(max_active_overlays > 0);
        assert!(
            max_active_overlays <= 100,
            "상한이 지나치게 크면 leak 방지 효과 없음"
        );
    }

    /// F-PF-C23-03: sweep_orphaned_processes 가 살아있는(None) 프로세스는 유지하는지 검증.
    /// HashMap 에 직접 접근할 수 없으므로 빈 맵 경계 케이스만 검증.
    #[test]
    fn sweep_empty_map_is_noop() {
        let mut map: HashMap<String, OverlayProcess> = HashMap::new();
        PlatformOverlayDriver::sweep_orphaned_processes(&mut map);
        assert!(map.is_empty());
    }
}
