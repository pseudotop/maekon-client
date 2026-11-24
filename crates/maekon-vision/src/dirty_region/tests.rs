use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use maekon_core::config::AppConfig;
use maekon_core::error::CoreError;
use maekon_core::models::context::{WindowBounds, WindowInfo};
use maekon_core::models::dirty_region::DirtyRect;
use maekon_core::ports::dirty_region::{DirtyRegionSource, ScreenChangeSource};
use maekon_core::ports::vision::{
    capture_denied, CaptureAccess, CaptureAuthority, CaptureGuard, CaptureObservationError,
    CaptureObserver, CapturePermit, CaptureScene, CaptureWindow,
};

use super::mask::ScreenRect;
use super::{screen_change_source, x11_session, ChangeListener, ListenerChanges};

/// Grants permits until `refuses` is set; `revoked` fails their checks.
#[derive(Default)]
struct Switches {
    refuses: AtomicBool,
    revoked: AtomicBool,
    revoke_on_retain: AtomicBool,
}

struct Guard {
    switches: Arc<Switches>,
    config: Option<Arc<AppConfig>>,
}

impl CaptureGuard for Guard {
    fn check(&self, access: CaptureAccess) -> Result<(), CoreError> {
        if matches!(access, CaptureAccess::Retain)
            && self.switches.revoke_on_retain.load(Ordering::SeqCst)
        {
            self.switches.revoked.store(true, Ordering::SeqCst);
        }
        if self.switches.revoked.load(Ordering::SeqCst) {
            return Err(capture_denied("synthetic revocation"));
        }
        Ok(())
    }

    fn display_id(&self) -> Option<u32> {
        Some(7)
    }

    fn config_snapshot(&self) -> Option<Arc<AppConfig>> {
        self.config.clone()
    }
}

struct Authority {
    switches: Arc<Switches>,
    config: Option<Arc<AppConfig>>,
}

impl CaptureAuthority for Authority {
    fn authorize(&self, _expected: Option<&WindowInfo>) -> Result<CapturePermit, CoreError> {
        if self.switches.refuses.load(Ordering::SeqCst) {
            return Err(capture_denied("synthetic consent withdrawn"));
        }
        Ok(CapturePermit::new(Arc::new(Guard {
            switches: self.switches.clone(),
            config: self.config.clone(),
        })))
    }
}

/// Replays scenes in order, repeating the last; `None` fails the observation.
struct Scenes {
    scenes: Mutex<Vec<Option<CaptureScene>>>,
    calls: AtomicUsize,
    displays: Mutex<Vec<Option<u32>>>,
}

impl Scenes {
    fn new(scenes: Vec<Option<CaptureScene>>) -> Arc<Self> {
        Arc::new(Self {
            scenes: Mutex::new(scenes),
            calls: AtomicUsize::new(0),
            displays: Mutex::new(Vec::new()),
        })
    }
}

impl CaptureObserver for Scenes {
    fn observe(&self, display_id: Option<u32>) -> Result<CaptureScene, CaptureObservationError> {
        self.displays.lock().unwrap().push(display_id);
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let scenes = self.scenes.lock().unwrap();
        scenes
            .get(call)
            .or_else(|| scenes.last())
            .cloned()
            .flatten()
            .ok_or(CaptureObservationError::EnumerationFailed)
    }
}

/// Hands out queued changes; once stopped it discards them and fails.
#[derive(Default)]
struct Listener {
    queued: Mutex<Vec<ListenerChanges>>,
    stopped: AtomicBool,
    drains: AtomicUsize,
}

struct SharedListener(Arc<Listener>);

impl ChangeListener for SharedListener {
    fn drain(&self) -> Result<ListenerChanges, CoreError> {
        self.0.drains.fetch_add(1, Ordering::SeqCst);
        if self.0.stopped.load(Ordering::SeqCst) {
            return Err(capture_denied("listener stopped"));
        }
        let mut queued = self.0.queued.lock().unwrap();
        Ok(if queued.is_empty() {
            ListenerChanges::default()
        } else {
            queued.remove(0)
        })
    }

    fn stop(&self) {
        self.0.stopped.store(true, Ordering::SeqCst);
        self.0.queued.lock().unwrap().clear();
    }

    fn name(&self) -> &str {
        "fake-listener"
    }
}

fn bounds(x: i32, y: i32, width: u32, height: u32) -> WindowBounds {
    WindowBounds {
        x,
        y,
        width,
        height,
    }
}

fn window(app: &str, bounds: WindowBounds) -> CaptureWindow {
    CaptureWindow {
        native_id: 1,
        pid: 1,
        app_name: app.to_string(),
        title: String::new(),
        bounds,
    }
}

/// A 200 x 100 display at x = 1000, with an editor in front and `others`.
fn scene(others: Vec<CaptureWindow>) -> Option<CaptureScene> {
    Some(CaptureScene {
        foreground: window("Editor", bounds(1000, 0, 100, 100)),
        display_id: 7,
        display_bounds: bounds(1000, 0, 200, 100),
        windows: others,
    })
}

fn vault() -> CaptureWindow {
    window("Vault", bounds(1100, 0, 50, 50))
}

fn config_excluding_vault() -> Option<Arc<AppConfig>> {
    let mut config = AppConfig::default_config();
    config.privacy.excluded_apps = vec!["Vault".to_string()];
    Some(Arc::new(config))
}

fn changes(rects: &[(i64, i64, i64, i64)], windows_changed: bool) -> ListenerChanges {
    ListenerChanges {
        rects: rects
            .iter()
            .map(|&(left, top, right, bottom)| ScreenRect {
                left,
                top,
                right,
                bottom,
            })
            .collect(),
        windows_changed,
    }
}

struct Rig {
    switches: Arc<Switches>,
    listener: Arc<Listener>,
    scenes: Arc<Scenes>,
    started: Arc<AtomicUsize>,
    source: ScreenChangeSource,
}

impl Rig {
    fn open(scenes: Vec<Option<CaptureScene>>, config: Option<Arc<AppConfig>>) -> Self {
        Self::open_with(scenes, config, Arc::default(), true)
    }

    fn open_with(
        scenes: Vec<Option<CaptureScene>>,
        config: Option<Arc<AppConfig>>,
        switches: Arc<Switches>,
        platform_supported: bool,
    ) -> Self {
        let listener = Arc::new(Listener::default());
        let scenes = Scenes::new(scenes);
        let started = Arc::new(AtomicUsize::new(0));
        let source = screen_change_source(
            {
                let listener = listener.clone();
                let started = started.clone();
                move |permit: CapturePermit| {
                    started.fetch_add(1, Ordering::SeqCst);
                    permit.check(CaptureAccess::Retain).unwrap();
                    platform_supported
                        .then(|| Box::new(SharedListener(listener)) as Box<dyn ChangeListener>)
                }
            },
            Arc::new(Authority {
                switches: switches.clone(),
                config,
            }),
            scenes.clone(),
        );
        Self {
            switches,
            listener,
            scenes,
            started,
            source,
        }
    }

    fn regions(&self) -> Arc<dyn DirtyRegionSource> {
        match &self.source {
            ScreenChangeSource::Regions(source) => source.clone(),
            ScreenChangeSource::CaptureComparison => panic!("expected OS regions"),
        }
    }

    fn queue(&self, changes: ListenerChanges) {
        self.listener.queued.lock().unwrap().push(changes);
    }
}

fn rect(left: u32, top: u32, right: u32, bottom: u32) -> DirtyRect {
    DirtyRect {
        left,
        top,
        right,
        bottom,
    }
}

fn assert_not_observed(result: Result<maekon_core::models::dirty_region::DirtyRegions, CoreError>) {
    let error = result.unwrap_err();
    assert!(
        matches!(error, CoreError::PermissionDenied { .. }),
        "expected not observed, got {error:?}"
    );
}

#[test]
fn changes_are_reported_in_display_pixels_and_clipped_to_the_display() {
    let rig = Rig::open(vec![scene(vec![])], None);
    let source = rig.regions();
    assert_eq!(source.name(), "fake-listener");
    rig.queue(changes(&[(990, 10, 1010, 20), (1300, 0, 1400, 10)], false));

    let regions = source.take().unwrap();
    assert_eq!(regions.rects(), &[rect(0, 10, 10, 20)]);
    // The permit's display was observed, at open and at the take.
    assert_eq!(*rig.scenes.displays.lock().unwrap(), vec![Some(7), Some(7)]);

    // Nothing queued: observed and unchanged.
    assert!(source.take().unwrap().is_empty());
}

#[test]
fn an_excluded_window_on_screen_now_is_cut_out() {
    let rig = Rig::open(
        vec![scene(vec![]), scene(vec![vault()])],
        config_excluding_vault(),
    );
    rig.queue(changes(&[(1090, 0, 1160, 10)], false));
    let regions = rig.regions().take().unwrap();
    assert_eq!(
        regions.rects(),
        &[rect(90, 0, 100, 10), rect(150, 0, 160, 10)]
    );
}

#[test]
fn an_excluded_window_seen_at_the_start_of_the_interval_is_cut_out_too() {
    // The vault was on screen at the previous take and is gone now.
    let rig = Rig::open(
        vec![scene(vec![vault()]), scene(vec![])],
        config_excluding_vault(),
    );
    rig.queue(changes(&[(1100, 0, 1150, 50), (1160, 0, 1170, 10)], false));
    let regions = rig.regions().take().unwrap();
    assert_eq!(regions.rects(), &[rect(160, 0, 170, 10)]);

    // The next interval starts without it, so its area is reported again.
    rig.queue(changes(&[(1100, 0, 1150, 50)], false));
    let regions = rig.regions().take().unwrap();
    assert_eq!(regions.rects(), &[rect(100, 0, 150, 50)]);
}

#[test]
fn an_excluded_app_in_front_is_cut_out() {
    let mut front = scene(vec![]).unwrap();
    front.foreground = window("Vault", bounds(1000, 0, 100, 100));
    let rig = Rig::open(vec![Some(front)], config_excluding_vault());
    rig.queue(changes(&[(1000, 0, 1200, 100)], false));
    let regions = rig.regions().take().unwrap();
    assert_eq!(regions.rects(), &[rect(100, 0, 200, 100)]);
}

#[test]
fn without_a_config_snapshot_the_default_policy_still_excludes_sensitive_apps() {
    let rig = Rig::open(
        vec![scene(vec![window("1Password", bounds(1000, 0, 200, 100))])],
        None,
    );
    rig.queue(changes(&[(1000, 0, 1010, 10)], false));
    assert!(rig.regions().take().unwrap().is_empty());
}

#[test]
fn moved_or_new_windows_make_the_interval_not_observed() {
    let rig = Rig::open(vec![scene(vec![])], None);
    let source = rig.regions();
    rig.queue(changes(&[(1000, 0, 1010, 10)], true));
    assert_not_observed(source.take());
    // Not a stop: the next interval is reported again.
    rig.queue(changes(&[(1000, 0, 1010, 10)], false));
    assert_eq!(source.take().unwrap().rects(), &[rect(0, 0, 10, 10)]);
}

#[test]
fn a_window_moved_away_is_still_cut_out_after_a_not_observed_interval() {
    // The vault is on screen while windows change, then gone: the first
    // interval is not observed and the next one still cuts out its area.
    let rig = Rig::open(
        vec![scene(vec![]), scene(vec![vault()]), scene(vec![])],
        config_excluding_vault(),
    );
    let source = rig.regions();
    rig.queue(changes(&[(1100, 0, 1110, 10)], true));
    assert_not_observed(source.take());
    rig.queue(changes(&[(1100, 0, 1110, 10)], false));
    assert!(source.take().unwrap().is_empty());
}

#[test]
fn an_unreadable_scene_is_not_observed_and_drops_what_was_collected() {
    let rig = Rig::open(vec![scene(vec![]), None, scene(vec![])], None);
    let source = rig.regions();
    rig.queue(changes(&[(1000, 0, 1010, 10)], false));
    assert_not_observed(source.take());
    assert!(source.take().unwrap().is_empty());
}

#[test]
fn a_denied_permit_stops_the_listener_for_good() {
    let rig = Rig::open(vec![scene(vec![])], None);
    let source = rig.regions();
    rig.queue(changes(&[(1000, 0, 1010, 10)], false));
    rig.switches.refuses.store(true, Ordering::SeqCst);
    assert_not_observed(source.take());
    assert!(rig.listener.stopped.load(Ordering::SeqCst));
    assert_eq!(rig.listener.drains.load(Ordering::SeqCst), 0);

    // Capture allowed again: this source stays stopped.
    rig.switches.refuses.store(false, Ordering::SeqCst);
    assert_not_observed(source.take());
}

#[test]
fn a_permit_revoked_while_preparing_the_regions_is_not_reported() {
    let rig = Rig::open(vec![scene(vec![])], None);
    let source = rig.regions();
    rig.queue(changes(&[(1000, 0, 1010, 10)], false));
    rig.switches.revoke_on_retain.store(true, Ordering::SeqCst);
    assert_not_observed(source.take());
    // The next take is refused before draining, and that stops the listener.
    assert_not_observed(source.take());
    assert!(rig.listener.stopped.load(Ordering::SeqCst));
    assert_eq!(rig.listener.drains.load(Ordering::SeqCst), 1);
}

#[test]
fn an_unsupported_platform_falls_back_to_capture_comparison() {
    let rig = Rig::open_with(vec![scene(vec![])], None, Arc::default(), false);
    assert!(matches!(rig.source, ScreenChangeSource::CaptureComparison));
    assert_eq!(rig.started.load(Ordering::SeqCst), 1);
}

#[test]
fn no_permit_at_open_falls_back_without_starting_a_listener() {
    let switches = Arc::new(Switches::default());
    switches.refuses.store(true, Ordering::SeqCst);
    let rig = Rig::open_with(vec![scene(vec![])], None, switches, true);
    assert!(matches!(rig.source, ScreenChangeSource::CaptureComparison));
    assert_eq!(rig.started.load(Ordering::SeqCst), 0);

    let switches = Arc::new(Switches::default());
    switches.revoked.store(true, Ordering::SeqCst);
    let rig = Rig::open_with(vec![scene(vec![])], None, switches, true);
    assert!(matches!(rig.source, ScreenChangeSource::CaptureComparison));
    assert_eq!(rig.started.load(Ordering::SeqCst), 0);
}

#[test]
fn an_unreadable_scene_at_open_falls_back_without_starting_a_listener() {
    let rig = Rig::open(vec![None], None);
    assert!(matches!(rig.source, ScreenChangeSource::CaptureComparison));
    assert_eq!(rig.started.load(Ordering::SeqCst), 0);
}

#[test]
fn only_an_x11_session_without_wayland_uses_x11_damage() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    };
    assert!(x11_session(env(&[("DISPLAY", ":0")])));
    assert!(x11_session(env(&[
        ("DISPLAY", ":0"),
        ("XDG_SESSION_TYPE", "x11")
    ])));
    assert!(!x11_session(env(&[])));
    assert!(!x11_session(env(&[("DISPLAY", "")])));
    assert!(!x11_session(env(&[
        ("DISPLAY", ":0"),
        ("WAYLAND_DISPLAY", "wayland-0")
    ])));
    assert!(x11_session(env(&[
        ("DISPLAY", ":0"),
        ("WAYLAND_DISPLAY", "")
    ])));
    assert!(!x11_session(env(&[
        ("DISPLAY", ":0"),
        ("XDG_SESSION_TYPE", "wayland")
    ])));
}
