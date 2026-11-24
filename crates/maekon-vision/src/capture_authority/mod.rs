//! Live, revocable capture authority. The composition root supplies native
//! observations and the same clock/power state used by the capture scheduler.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local};
use maekon_core::capture_gate::capture_permitted_now_with_power;
use maekon_core::config::AppConfig;
use maekon_core::config_manager::ConfigManager;
use maekon_core::consent::ConsentPermissions;
use maekon_core::error::CoreError;
use maekon_core::models::context::{WindowBounds, WindowInfo};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::vision::{
    capture_denied, CaptureAccess, CaptureAuthority, CaptureGuard, CaptureObservationError,
    CaptureObserver, CapturePauseState, CapturePermit, CaptureScene, CaptureWindow,
};

use crate::privacy::capture_scene::{same_bounds, same_foreground};
use crate::privacy::validate_capture_scene;

/// Required runtime inputs, not a replacement policy decision. Capture policy
/// remains in `maekon_core::capture_gate`; callers cannot bypass it with a bool.
#[derive(Clone, Copy)]
pub struct CaptureEnvironment {
    pub now: DateTime<Local>,
    pub battery_saver_active: bool,
}

#[derive(Clone)]
pub struct LiveCaptureAuthority {
    state: Arc<LiveState>,
}

struct LiveState {
    config: ConfigManager,
    consent: Arc<dyn ConsentManagerPort>,
    pause: Arc<CapturePauseState>,
    observer: Arc<dyn CaptureObserver>,
    environment: Arc<dyn Fn() -> CaptureEnvironment + Send + Sync>,
    permission_epoch: Mutex<u64>,
}

struct PolicyBinding {
    config: Arc<AppConfig>,
    config_epoch: u64,
    consent_id: String,
    pause_epoch: u64,
}

struct LiveCaptureGuard {
    state: Arc<LiveState>,
    policy: PolicyBinding,
    permission_epoch: u64,
    foreground: CaptureWindow,
    display_id: u32,
    display_bounds: WindowBounds,
}

impl LiveCaptureAuthority {
    pub fn new(
        config: ConfigManager,
        consent: Arc<dyn ConsentManagerPort>,
        pause: Arc<CapturePauseState>,
        observer: Arc<dyn CaptureObserver>,
        environment: Arc<dyn Fn() -> CaptureEnvironment + Send + Sync>,
    ) -> Self {
        Self {
            state: Arc::new(LiveState {
                config,
                consent,
                pause,
                observer,
                environment,
                permission_epoch: Mutex::new(0),
            }),
        }
    }
}

impl LiveState {
    fn observe(&self, display_id: Option<u32>) -> Result<CaptureScene, CoreError> {
        match self.observer.observe(display_id) {
            Ok(scene) => Ok(scene),
            Err(CaptureObservationError::PermissionDenied) => {
                let mut epoch = self
                    .permission_epoch
                    .lock()
                    .map_err(|_| capture_denied("capture permission state poisoned"))?;
                *epoch = epoch.saturating_add(1);
                drop(epoch);
                Err(capture_denied("native capture permission unavailable"))
            }
            Err(_) => Err(capture_denied("native capture scene unavailable")),
        }
    }

    fn check_policy(&self, binding: &PolicyBinding) -> Result<ConsentPermissions, CoreError> {
        self.pause.check(binding.pause_epoch)?;
        // ConfigManager binds its policy and epoch under one watch lock. A
        // policy ABA revokes old images; unrelated settings keep them valid.
        if self.config.capture_policy_snapshot()?.1 != binding.config_epoch {
            return Err(capture_denied("capture policy changed"));
        }
        let record = self
            .consent
            .effective_consent()
            .ok_or_else(|| capture_denied("capture consent unavailable"))?;
        if record.consent_id != binding.consent_id {
            return Err(capture_denied("capture consent changed"));
        }
        let environment = (self.environment)();
        if self.consent.erasing().load(Ordering::Acquire)
            || !capture_permitted_now_with_power(
                &binding.config,
                &record.permissions,
                false,
                environment.battery_saver_active,
                environment.now,
            )
        {
            return Err(capture_denied("capture policy disallows access"));
        }
        Ok(record.permissions)
    }

    fn permission_epoch(&self) -> Result<u64, CoreError> {
        // Serialize permission observations so a denial seen by any permit
        // invalidates all earlier permits, including after permission returns.
        // Native adapters must make this a bounded, non-interactive check.
        let mut epoch = self
            .permission_epoch
            .lock()
            .map_err(|_| capture_denied("capture permission state poisoned"))?;
        if *epoch == u64::MAX {
            return Err(capture_denied("capture permission epoch exhausted"));
        }
        if self.observer.check_permission().is_err() {
            *epoch = epoch.saturating_add(1);
            drop(epoch);
            return Err(capture_denied("native capture permission unavailable"));
        }
        Ok(*epoch)
    }
}

impl CaptureAuthority for LiveCaptureAuthority {
    fn authorize(&self, expected: Option<&WindowInfo>) -> Result<CapturePermit, CoreError> {
        let (config, config_epoch) = self.state.config.capture_policy_snapshot()?;
        let policy = PolicyBinding {
            config,
            config_epoch,
            consent_id: self
                .state
                .consent
                .effective_consent()
                .ok_or_else(|| capture_denied("capture consent unavailable"))?
                .consent_id,
            pause_epoch: self.state.pause.snapshot()?,
        };
        self.state.check_policy(&policy)?;
        let permission_epoch = self.state.permission_epoch()?;
        let scene = self.state.observe(None)?;
        validate_capture_scene(&policy.config.privacy, &scene)?;
        if expected.is_some_and(|window| {
            window.pid != scene.foreground.pid
                || window.app_name != scene.foreground.app_name
                || window.title != scene.foreground.title
                || window
                    .bounds
                    .is_some_and(|bounds| !same_bounds(bounds, scene.foreground.bounds))
        }) {
            return Err(capture_denied("foreground observation changed"));
        }
        let guard = LiveCaptureGuard {
            state: self.state.clone(),
            policy,
            permission_epoch,
            foreground: scene.foreground,
            display_id: scene.display_id,
            display_bounds: scene.display_bounds,
        };
        // Native observation is outside policy locks. A revoke/pause/config
        // publication during it must not mint an already invalid permit.
        guard.check(CaptureAccess::Retain)?;
        Ok(CapturePermit::new(Arc::new(guard)))
    }

    fn toggle_pause(&self) -> Result<bool, CoreError> {
        self.state.pause.toggle()
    }
}

impl CaptureGuard for LiveCaptureGuard {
    fn check(&self, access: CaptureAccess) -> Result<(), CoreError> {
        self.state.check_policy(&self.policy)?;
        if self.state.permission_epoch()? != self.permission_epoch {
            return Err(capture_denied("native capture permission changed"));
        }
        if let CaptureAccess::Acquire { display_id } = access {
            if display_id != self.display_id {
                return Err(capture_denied("capture display changed"));
            }
        }
        if matches!(
            access,
            CaptureAccess::Acquire { .. } | CaptureAccess::CurrentScene
        ) {
            let scene = self.state.observe(Some(self.display_id))?;
            validate_capture_scene(&self.policy.config.privacy, &scene)?;
            if scene.display_id != self.display_id
                || !same_bounds(scene.display_bounds, self.display_bounds)
                || !same_foreground(&scene.foreground, &self.foreground)
            {
                return Err(capture_denied("capture scene changed"));
            }
        }
        let permissions = self.state.check_policy(&self.policy)?;
        if (matches!(access, CaptureAccess::Ocr) && !permissions.ocr_processing)
            || (matches!(access, CaptureAccess::FullText) && !permissions.full_text_extraction)
        {
            return Err(capture_denied("capture text consent unavailable"));
        }
        if self.state.permission_epoch()? != self.permission_epoch {
            return Err(capture_denied("native capture permission changed"));
        }
        Ok(())
    }

    fn display_id(&self) -> Option<u32> {
        Some(self.display_id)
    }

    fn config_snapshot(&self) -> Option<Arc<AppConfig>> {
        Some(self.policy.config.clone())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod flow_tests;
