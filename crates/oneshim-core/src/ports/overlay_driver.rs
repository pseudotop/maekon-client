//! Overlay driver port — defines the contract for rendering transparent
//! highlight overlays on screen elements (MagicOverlay, heatmap ghosts).
//! Implemented by Tauri WebView overlay in `src-tauri`.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::gui::{HighlightHandle, HighlightRequest};
use crate::models::ui_scene::UiScene;

/// # Errors
/// - `CoreError::Internal` (wire: `internal.generic`) for Tauri IPC
///   failures (webview disposed, channel closed, serialization panic).
/// - `CoreError::Io` (via `#[from] std::io::Error`, wire:
///   `internal.io`) for overlay payload file I/O when passing large
///   scene data between processes via temp file (iter-95 i/o pattern
///   fix; the platform_overlay.rs now uses `?` auto-conversion instead
///   of the io::Error::new rebuild anti-pattern).
/// - `ServiceUnavailable` (wire: `service.unavailable`) when running
///   on a platform without overlay support (no-op driver fallback).
#[async_trait]
pub trait OverlayDriver: Send + Sync {
    async fn show_highlights(&self, req: HighlightRequest) -> Result<HighlightHandle, CoreError>;

    async fn clear_highlights(&self, handle_id: &str) -> Result<(), CoreError>;

    /// Render all elements from a UiScene as detection overlay boxes.
    async fn show_detection(&self, scene: &UiScene) -> Result<(), CoreError>;

    /// Clear all detection overlay boxes.
    async fn clear_detection(&self) -> Result<(), CoreError>;
}
