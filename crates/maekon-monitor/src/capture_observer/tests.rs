use super::capture_scene_from_windows;
use maekon_core::models::context::WindowBounds;
use maekon_core::ports::vision::{CaptureDisplay, CaptureObservationError, CaptureWindow};

fn bounds(x: i32, y: i32, width: u32, height: u32) -> WindowBounds {
    WindowBounds {
        x,
        y,
        width,
        height,
    }
}

fn coordinates(rectangle: WindowBounds) -> (i32, i32, u32, u32) {
    (rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

fn displays() -> Vec<CaptureDisplay> {
    vec![
        CaptureDisplay {
            id: 71,
            bounds: bounds(-3840, 0, 3840, 2160),
        },
        CaptureDisplay {
            id: 93,
            bounds: bounds(0, 0, 1920, 1080),
        },
    ]
}

fn window(id: u64, rectangle: WindowBounds) -> CaptureWindow {
    CaptureWindow {
        native_id: id,
        pid: 41,
        app_name: "synthetic.exe".into(),
        title: "합성 창".into(),
        bounds: rectangle,
    }
}

fn foreground() -> CaptureWindow {
    window(5, bounds(-2000, 100, 900, 600))
}

#[test]
fn scene_preserves_foreground_and_untitled_background_on_exact_display() {
    let active = foreground();
    let mut background = window(9, bounds(-100, 50, 200, 200));
    background.app_name = "sensitive.exe".into();
    background.title.clear();
    let other = window(12, bounds(100, 100, 800, 600));
    let scene = capture_scene_from_windows(
        &displays(),
        vec![background, other, active.clone()],
        active.native_id,
        None,
    )
    .unwrap();
    assert_eq!(scene.display_id, 71);
    assert_eq!(coordinates(scene.display_bounds), (-3840, 0, 3840, 2160));
    assert_eq!(scene.foreground.native_id, active.native_id);
    assert_eq!(scene.foreground.pid, active.pid);
    assert_eq!(scene.foreground.app_name, active.app_name);
    assert_eq!(scene.foreground.title, active.title);
    assert_eq!(coordinates(scene.foreground.bounds), (-2000, 100, 900, 600));
    assert_eq!(
        scene
            .windows
            .iter()
            .map(|w| w.native_id)
            .collect::<Vec<_>>(),
        vec![9, 5]
    );
    assert_eq!(scene.windows[0].app_name, "sensitive.exe");
    assert!(scene.windows[0].title.is_empty());
}

#[test]
fn requested_display_is_exact_and_must_intersect_foreground() {
    let spanning = window(5, bounds(-100, 100, 200, 300));
    let scene = capture_scene_from_windows(&displays(), vec![spanning], 5, Some(93)).unwrap();
    assert_eq!(scene.display_id, 93);
    assert_eq!(coordinates(scene.display_bounds), (0, 0, 1920, 1080));
    assert_eq!(scene.windows.len(), 1);
    for requested in [Some(0), Some(72), Some(93)] {
        assert_eq!(
            capture_scene_from_windows(&displays(), vec![foreground()], 5, requested).unwrap_err(),
            CaptureObservationError::GeometryUnavailable
        );
    }
}

#[test]
fn absent_or_unknown_foreground_is_not_an_empty_success() {
    for (windows, id) in [
        (vec![], 5),
        (vec![foreground()], 0),
        (vec![foreground()], 9),
    ] {
        assert_eq!(
            capture_scene_from_windows(&displays(), windows, id, None).unwrap_err(),
            CaptureObservationError::ForegroundUnavailable
        );
    }
}

#[test]
fn native_identities_must_be_present_and_unique() {
    for invalid in [
        window(0, bounds(100, 100, 100, 100)),
        window(5, bounds(100, 100, 100, 100)),
    ] {
        assert_eq!(
            capture_scene_from_windows(&displays(), vec![foreground(), invalid], 5, None)
                .unwrap_err(),
            CaptureObservationError::EnumerationFailed
        );
    }
}

#[test]
fn unknown_geometry_cannot_be_assumed_to_belong_to_another_display() {
    for rectangle in [bounds(100, 100, 0, 10), bounds(100, 100, 10, 0)] {
        assert_eq!(
            capture_scene_from_windows(
                &displays(),
                vec![foreground(), window(9, rectangle)],
                5,
                None,
            )
            .unwrap_err(),
            CaptureObservationError::GeometryUnavailable
        );
    }
}

#[test]
fn unidentified_visible_owners_deny_but_other_displays_do_not() {
    let mut missing_pid = window(9, bounds(-100, 50, 200, 200));
    missing_pid.pid = 0;
    let mut missing_name = missing_pid.clone();
    missing_name.pid = 42;
    missing_name.app_name = " \t\n".into();
    for mut unknown in [missing_pid, missing_name] {
        assert_eq!(
            capture_scene_from_windows(&displays(), vec![foreground(), unknown.clone()], 5, None)
                .unwrap_err(),
            CaptureObservationError::OwnerUnavailable
        );
        unknown.bounds = bounds(0, 50, 200, 200);
        let scene =
            capture_scene_from_windows(&displays(), vec![foreground(), unknown], 5, None).unwrap();
        assert_eq!(scene.windows.len(), 1);
        assert_eq!(scene.windows[0].native_id, 5);
    }
}

#[test]
fn foreground_owner_is_validated_as_well_as_background_owners() {
    let mut active = foreground();
    active.pid = 0;
    assert_eq!(
        capture_scene_from_windows(&displays(), vec![active], 5, None).unwrap_err(),
        CaptureObservationError::OwnerUnavailable
    );
}
