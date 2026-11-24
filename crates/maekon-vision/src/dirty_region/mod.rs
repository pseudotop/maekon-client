//! Authorized OS change regions: what an operating-system listener reports
//! as changed, after the capture policy and the excluded-app policy.
//!
//! A [`ChangeListener`] collects the platform's change notifications on its
//! own thread and stops itself once its capture permit is revoked.
//! [`AuthorizedDirtyRegions`] is the only way its rectangles leave this crate:
//!
//! - every take asks the capture authority for a fresh permit; a denial
//!   stops the listener for good, so a caller opens a new source once
//!   capture is allowed again;
//! - rectangles over windows of excluded apps are cut out, using the excluded
//!   windows seen at both ends of the interval;
//! - when top-level windows moved, appeared or disappeared during the
//!   interval, where excluded windows were in between is unknown, so the
//!   take reports "not observed" instead of guessing; so does a take of
//!   what was drawn within [`pending::LAYOUT_SETTLE`] after such a change,
//!   which can arrive after the change itself.

#[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
pub mod macos;
pub mod macos_rules;
pub mod mask;
pub mod pending;
#[cfg(target_os = "windows")]
pub mod windows;
pub mod windows_rules;
#[cfg(target_os = "linux")]
pub mod x11;

use std::sync::{Arc, Mutex};

use maekon_core::config::PrivacyConfig;
use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;
use maekon_core::models::dirty_region::DirtyRegions;
use maekon_core::ports::dirty_region::{DirtyRegionSource, ScreenChangeSource};
use maekon_core::ports::vision::{
    capture_denied, CaptureAccess, CaptureAuthority, CaptureObserver, CapturePermit, CaptureScene,
};

use crate::privacy::should_exclude_by_policy;
use mask::ScreenRect;

/// What a listener collected since its previous drain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListenerChanges {
    /// Changed rectangles in the platform's screen coordinates.
    pub rects: Vec<ScreenRect>,
    /// A top-level window appeared, disappeared, moved or was restacked, or
    /// the listener could not follow the screen.
    pub windows_changed: bool,
}

/// Collects OS change notifications on its own thread, under the permit it
/// was started with. Dropping a listener stops it.
pub trait ChangeListener: Send + Sync {
    /// What was collected since the previous call. Fails once the listener
    /// has stopped.
    fn drain(&self) -> Result<ListenerChanges, CoreError>;

    /// Stop listening, discard what was collected and release the OS
    /// resources.
    fn stop(&self);

    fn name(&self) -> &str;
}

/// [`DirtyRegionSource`] over one listener, checked against the capture
/// authority and the window scene on every take.
pub struct AuthorizedDirtyRegions {
    listener: Box<dyn ChangeListener>,
    authority: Arc<dyn CaptureAuthority>,
    observer: Arc<dyn CaptureObserver>,
    /// Excluded windows seen by the previous take, or at start.
    excluded: Mutex<Vec<ScreenRect>>,
}

impl AuthorizedDirtyRegions {
    fn take_authorized(&self, permit: &CapturePermit) -> Result<DirtyRegions, CoreError> {
        let changes = self.listener.drain()?;
        let (scene, excluded_now) = excluded_windows(&*self.observer, permit)?;
        let excluded_before = {
            let mut excluded = self
                .excluded
                .lock()
                .map_err(|_| internal("excluded window state poisoned"))?;
            std::mem::replace(&mut *excluded, excluded_now.clone())
        };
        if changes.windows_changed {
            return Err(capture_denied(
                "windows changed since the last take; excluded windows unknown in between",
            ));
        }
        let visible = mask::subtract_all(
            changes.rects,
            excluded_before.into_iter().chain(excluded_now),
        );
        let display = ScreenRect::from_bounds(&scene.display_bounds);
        let regions = DirtyRegions::new(
            scene.display_bounds.width,
            scene.display_bounds.height,
            visible.iter().map(|rect| rect.on_display(&display)),
        )
        .map_err(|error| internal(&format!("display geometry invalid: {error}")))?;
        // A permit revoked while the regions were prepared invalidates them.
        permit.check(CaptureAccess::Retain)?;
        Ok(regions)
    }
}

impl DirtyRegionSource for AuthorizedDirtyRegions {
    fn take(&self) -> Result<DirtyRegions, CoreError> {
        match authorize(&*self.authority) {
            Ok(permit) => self.take_authorized(&permit),
            Err(error) => {
                self.listener.stop();
                Err(error)
            }
        }
    }

    fn name(&self) -> &str {
        self.listener.name()
    }
}

/// Use OS regions when the platform listener starts under a fresh permit;
/// capture comparison otherwise. `start_listener` returns `None` where the
/// platform or session has no change notifications.
pub fn screen_change_source(
    start_listener: impl FnOnce(CapturePermit) -> Option<Box<dyn ChangeListener>>,
    authority: Arc<dyn CaptureAuthority>,
    observer: Arc<dyn CaptureObserver>,
) -> ScreenChangeSource {
    let started = authorize(&*authority).and_then(|permit| {
        let (_, excluded) = excluded_windows(&*observer, &permit)?;
        Ok(
            start_listener(permit).map(|listener| AuthorizedDirtyRegions {
                listener,
                authority,
                observer,
                excluded: Mutex::new(excluded),
            }),
        )
    });
    match started {
        Ok(Some(source)) => ScreenChangeSource::Regions(Arc::new(source)),
        Ok(None) => ScreenChangeSource::CaptureComparison,
        Err(error) => {
            tracing::debug!(%error, "OS change regions unavailable; using capture comparison");
            ScreenChangeSource::CaptureComparison
        }
    }
}

/// Whether the environment describes an X11 session. A Wayland session's
/// Xwayland root shows only X11 clients, so DAMAGE there would miss changes.
pub fn x11_session(var: impl Fn(&str) -> Option<String>) -> bool {
    let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
    set("DISPLAY")
        && !set("WAYLAND_DISPLAY")
        && var("XDG_SESSION_TYPE").as_deref() != Some("wayland")
}

/// A fresh permit that the current scene satisfies.
fn authorize(authority: &dyn CaptureAuthority) -> Result<CapturePermit, CoreError> {
    let permit = authority.authorize(None)?;
    permit.check(CaptureAccess::CurrentScene)?;
    Ok(permit)
}

/// The scene of the permit's display and the bounds of its excluded windows.
fn excluded_windows(
    observer: &dyn CaptureObserver,
    permit: &CapturePermit,
) -> Result<(CaptureScene, Vec<ScreenRect>), CoreError> {
    let scene = observer
        .observe(permit.display_id())
        .map_err(|error| capture_denied(&format!("window scene unavailable: {error:?}")))?;
    let privacy = permit
        .config_snapshot()
        .map_or_else(PrivacyConfig::default, |config| config.privacy.clone());
    let excluded = std::iter::once(&scene.foreground)
        .chain(&scene.windows)
        .filter(|window| should_exclude_by_policy(&privacy, &window.app_name, &window.title))
        .map(|window| ScreenRect::from_bounds(&window.bounds))
        .collect();
    Ok((scene, excluded))
}

fn internal(message: &str) -> CoreError {
    CoreError::Internal {
        code: InternalCode::Generic,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests;
