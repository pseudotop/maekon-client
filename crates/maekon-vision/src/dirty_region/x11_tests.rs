//! Drives the listener against a private Xvfb display. Each test skips, with
//! a note on stderr, where Xvfb is not installed.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use maekon_core::error::CoreError;
use maekon_core::ports::vision::{capture_denied, CaptureAccess, CaptureGuard, CapturePermit};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateGCAux, CreateWindowAux, Rectangle, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use super::super::mask::ScreenRect;
use super::super::pending::LAYOUT_SETTLE;
use super::super::{ChangeListener, ListenerChanges};
use super::X11DamageListener;

struct Xvfb {
    child: Child,
    display: String,
}

impl Xvfb {
    /// A fresh 320 x 240 display, or `None` (test skipped) without Xvfb.
    /// Xvfb picks a free display number itself (`-displayfd`), so parallel
    /// tests never share a server, and `-noreset` keeps it from resetting
    /// whenever its last client disconnects.
    fn start() -> Option<Self> {
        let mut child = match Command::new("Xvfb")
            .args(["-displayfd", "1", "-screen", "0", "320x240x24"])
            .args(["-nolisten", "tcp", "-noreset"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                eprintln!("skipping X11 DAMAGE test: cannot run Xvfb ({error})");
                return None;
            }
        };
        // The number arrives once the server accepts connections.
        let mut line = String::new();
        let read = child
            .stdout
            .take()
            .map(|out| BufReader::new(out).read_line(&mut line));
        let mut xvfb = Self {
            child,
            display: format!(":{}", line.trim()),
        };
        if matches!(read, Some(Ok(_))) && line.trim().parse::<u32>().is_ok() {
            return Some(xvfb);
        }
        xvfb.terminate();
        eprintln!("skipping X11 DAMAGE test: Xvfb did not start");
        None
    }

    /// SIGTERM, so Xvfb removes its lock file and socket.
    fn terminate(&mut self) {
        let terminated = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .is_ok_and(|status| status.success());
        if !terminated {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[derive(Default)]
struct Switch(AtomicBool);

impl CaptureGuard for Switch {
    fn check(&self, _access: CaptureAccess) -> Result<(), CoreError> {
        if self.0.load(Ordering::SeqCst) {
            Err(capture_denied("synthetic revocation"))
        } else {
            Ok(())
        }
    }
}

fn permit(revoked: &Arc<Switch>) -> CapturePermit {
    CapturePermit::new(revoked.clone())
}

/// A second client that maps a window and draws into it.
struct Painter {
    conn: RustConnection,
    root: Window,
}

impl Painter {
    fn connect(display: &str) -> Self {
        let (conn, screen) = x11rb::connect(Some(display)).unwrap();
        let root = conn.setup().roots[screen].root;
        Self { conn, root }
    }

    fn map_window(&self, x: i16, y: i16, width: u16, height: u16) -> Window {
        let screen = &self.conn.setup().roots[0];
        let window = self.conn.generate_id().unwrap();
        self.conn
            .create_window(
                screen.root_depth,
                window,
                self.root,
                x,
                y,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new()
                    .background_pixel(screen.black_pixel)
                    .override_redirect(1),
            )
            .unwrap();
        self.conn.map_window(window).unwrap();
        self.conn.flush().unwrap();
        window
    }

    fn fill(&self, window: Window, rect: Rectangle) {
        let screen = &self.conn.setup().roots[0];
        let gc = self.conn.generate_id().unwrap();
        self.conn
            .create_gc(
                gc,
                window,
                &CreateGCAux::new().foreground(screen.white_pixel),
            )
            .unwrap();
        self.conn.poly_fill_rectangle(window, gc, &[rect]).unwrap();
        self.conn.free_gc(gc).unwrap();
        self.conn.flush().unwrap();
        // A round trip, so the server has drawn before the test reads.
        self.conn.get_input_focus().unwrap().reply().unwrap();
    }
}

/// Long enough for the listener thread to handle what a local Xvfb sends
/// right after start. Shorter on a slow machine only weakens a test.
const SETTLE: Duration = Duration::from_millis(300);

/// Drain until `done` holds for everything collected so far (2 s bound).
fn collect(
    listener: &X11DamageListener,
    done: impl Fn(&ListenerChanges) -> bool,
) -> ListenerChanges {
    let mut all = ListenerChanges::default();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let changes = listener.drain().unwrap();
        all.rects.extend(changes.rects);
        all.windows_changed |= changes.windows_changed;
        if done(&all) || Instant::now() > deadline {
            return all;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn covers(rects: &[ScreenRect], target: ScreenRect) -> bool {
    let rest = super::super::mask::subtract_all(vec![target], rects.iter().copied());
    rest.is_empty()
}

fn wait_finished(listener: &X11DamageListener) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !listener.is_finished() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

#[test]
fn reports_drawn_rectangles_and_window_changes() {
    let Some(xvfb) = Xvfb::start() else { return };
    let revoked = Arc::new(Switch::default());
    let listener = X11DamageListener::start(Some(xvfb.display.as_str()), permit(&revoked)).unwrap();
    assert_eq!(listener.name(), "x11-damage");
    let painter = Painter::connect(&xvfb.display);

    let window = painter.map_window(10, 20, 50, 40);
    let mapped = collect(&listener, |all| {
        all.windows_changed && !all.rects.is_empty()
    });
    assert!(
        mapped.windows_changed,
        "mapping a window is a window change"
    );
    let area = ScreenRect {
        left: 10,
        top: 20,
        right: 60,
        bottom: 60,
    };
    assert!(covers(&mapped.rects, area), "{:?}", mapped.rects);

    // Drawing within the settle period of a window change counts as part of
    // it (`pending::LAYOUT_SETTLE`).
    std::thread::sleep(LAYOUT_SETTLE);
    painter.fill(
        window,
        Rectangle {
            x: 5,
            y: 5,
            width: 10,
            height: 10,
        },
    );
    let drawn = ScreenRect {
        left: 15,
        top: 25,
        right: 25,
        bottom: 35,
    };
    let painted = collect(&listener, |all| covers(&all.rects, drawn));
    assert!(covers(&painted.rects, drawn), "{:?}", painted.rects);
    assert!(!painted.windows_changed, "drawing alone moves no window");
    // DAMAGE reports where drawing happened, not the whole screen.
    let outside = ScreenRect {
        left: 200,
        top: 0,
        right: 320,
        bottom: 240,
    };
    assert!(painted
        .rects
        .iter()
        .all(|rect| super::super::mask::subtract_all(vec![*rect], [outside]) == vec![*rect]));
}

#[test]
fn starting_to_listen_is_not_a_change() {
    let Some(xvfb) = Xvfb::start() else { return };
    let revoked = Arc::new(Switch::default());
    let listener = X11DamageListener::start(Some(xvfb.display.as_str()), permit(&revoked)).unwrap();
    std::thread::sleep(SETTLE);
    assert_eq!(listener.drain().unwrap(), ListenerChanges::default());
}

#[test]
fn a_revoked_permit_stops_the_thread_at_the_next_event() {
    let Some(xvfb) = Xvfb::start() else { return };
    let revoked = Arc::new(Switch::default());
    let listener = X11DamageListener::start(Some(xvfb.display.as_str()), permit(&revoked)).unwrap();
    let painter = Painter::connect(&xvfb.display);
    let window = painter.map_window(0, 0, 40, 40);
    collect(&listener, |all| !all.rects.is_empty());

    revoked.0.store(true, Ordering::SeqCst);
    painter.fill(
        window,
        Rectangle {
            x: 0,
            y: 0,
            width: 5,
            height: 5,
        },
    );
    assert!(
        wait_finished(&listener),
        "the thread outlived the revocation"
    );
    assert!(matches!(
        listener.drain().unwrap_err(),
        CoreError::PermissionDenied { .. }
    ));
}

#[test]
fn stop_wakes_an_idle_thread_and_drop_stops_it_too() {
    let Some(xvfb) = Xvfb::start() else { return };
    let revoked = Arc::new(Switch::default());
    let listener = X11DamageListener::start(Some(xvfb.display.as_str()), permit(&revoked)).unwrap();
    // Let the thread handle its start-up events and block, so only the
    // wake-up from `stop` can end it.
    std::thread::sleep(SETTLE);
    listener.stop();
    assert!(wait_finished(&listener), "stop left the thread blocked");
    assert!(matches!(
        listener.drain().unwrap_err(),
        CoreError::PermissionDenied { .. }
    ));
    listener.stop();

    let dropped = X11DamageListener::start(Some(xvfb.display.as_str()), permit(&revoked)).unwrap();
    let pending = dropped.pending.clone();
    drop(dropped);
    assert!(
        super::lock(&pending).is_stopped(),
        "drop left the listener running"
    );
}

#[test]
fn no_x_server_means_no_listener() {
    let revoked = Arc::new(Switch::default());
    assert!(X11DamageListener::start(Some(":999"), permit(&revoked)).is_none());
}
