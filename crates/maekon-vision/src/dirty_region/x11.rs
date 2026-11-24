//! X11 DAMAGE change listener.
//!
//! One pure-Rust `RustConnection` watches the root window. DAMAGE at the
//! `NonEmpty` level reports once when anything is drawn; the listener then
//! moves the accumulated damage into an XFIXES region and reads its
//! rectangles. `SubstructureNotify` on the root reports top-level windows
//! being mapped, unmapped, moved or restacked, which marks the interval as
//! `windows_changed`. The whole-root damage that creating the damage object
//! reports is discarded at start, so starting to listen is not a change.
//!
//! The listener thread checks its capture permit after every event and stops
//! on the first refusal. [`X11DamageListener::stop`] wakes the thread with a
//! client message to a private window, so the thread does not stay blocked in
//! `wait_for_event` after a stop.
//!
//! This file needs an X server, so the mutation gates leave it out
//! (`.cargo/mutants.toml`); `x11_tests.rs` drives it against Xvfb and skips
//! where Xvfb is not installed. The logic it relies on lives in measured
//! files: [`super::pending`], [`super::mask`] and [`super::x11_session`].

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use maekon_core::error::CoreError;
use maekon_core::models::context::WindowBounds;
use maekon_core::ports::vision::{CaptureAccess, CapturePermit};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _, CreateWindowAux,
    EventMask, Rectangle, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use super::mask::ScreenRect;
use super::pending::PendingChanges;
use super::{x11_session, ChangeListener, ListenerChanges};

/// The X11 listener for this session under `permit`, or `None` outside an
/// X11 session or without DAMAGE and XFIXES. Connecting has no timeout, so
/// call this from a blocking context.
pub fn session_listener(permit: CapturePermit) -> Option<Box<dyn ChangeListener>> {
    if !x11_session(|name| std::env::var(name).ok()) {
        return None;
    }
    X11DamageListener::start(None, permit).map(|listener| Box::new(listener) as _)
}

/// [`ChangeListener`] over the X11 DAMAGE extension on the root window.
pub struct X11DamageListener {
    pending: Arc<Mutex<PendingChanges>>,
    conn: Arc<RustConnection>,
    wake: Window,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl X11DamageListener {
    /// Listen on `display` (`None` reads `$DISPLAY`) under `permit`. `None`
    /// when there is no X server there, or it lacks DAMAGE or XFIXES.
    pub fn start(display: Option<&str>, permit: CapturePermit) -> Option<Self> {
        let (conn, screen) = x11rb::connect(display).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        conn.extension_information(damage::X11_EXTENSION_NAME)
            .ok()??;
        conn.extension_information(xfixes::X11_EXTENSION_NAME)
            .ok()??;
        conn.xfixes_query_version(5, 0).ok()?.reply().ok()?;
        conn.damage_query_version(1, 1).ok()?.reply().ok()?;

        let wake = conn.generate_id().ok()?;
        conn.create_window(
            0,
            wake,
            root,
            -1,
            -1,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new(),
        )
        .ok()?;
        let region = conn.generate_id().ok()?;
        conn.xfixes_create_region(region, &[]).ok()?;
        let damage = conn.generate_id().ok()?;
        conn.damage_create(damage, root, damage::ReportLevel::NON_EMPTY)
            .ok()?;
        // Creating a damage object on a window damages all of it once.
        // Discard that, so the first drain reports only what changed since.
        conn.damage_subtract(damage, x11rb::NONE, x11rb::NONE)
            .ok()?;
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY),
        )
        .ok()?;
        // A round trip, so a rejected request fails here and not on the thread.
        conn.get_input_focus().ok()?.reply().ok()?;

        let conn = Arc::new(conn);
        let pending = Arc::new(Mutex::new(PendingChanges::default()));
        let thread = std::thread::Builder::new()
            .name("maekon-x11-damage".to_string())
            .spawn({
                let conn = conn.clone();
                let pending = pending.clone();
                move || listen(&conn, &pending, &permit, wake, damage, region)
            })
            .ok()?;
        Some(Self {
            pending,
            conn,
            wake,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Whether the listener thread has exited.
    pub fn is_finished(&self) -> bool {
        self.thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }
}

/// Collect damage and structure changes until stopped, the permit is
/// refused, or the connection fails; then release the server resources.
fn listen(
    conn: &RustConnection,
    pending: &Mutex<PendingChanges>,
    permit: &CapturePermit,
    wake: Window,
    damage: damage::Damage,
    region: xfixes::Region,
) {
    loop {
        let event = conn.wait_for_event();
        let refused = permit.check(CaptureAccess::Retain).is_err();
        let event = match event {
            Ok(event) if !refused && !lock(pending).is_stopped() => event,
            _ => {
                lock(pending).stop();
                break;
            }
        };
        match event {
            Event::DamageNotify(_) => {
                // Move the damage into `region` and read it; the next drawing
                // makes the damage non-empty again and is reported anew.
                let parts = conn
                    .damage_subtract(damage, x11rb::NONE, region)
                    .ok()
                    .and_then(|_| conn.xfixes_fetch_region(region).ok())
                    .and_then(|cookie| cookie.reply().ok());
                if !collect_damage(&mut lock(pending), parts) {
                    break;
                }
            }
            Event::MapNotify(_)
            | Event::UnmapNotify(_)
            | Event::ConfigureNotify(_)
            | Event::ReparentNotify(_)
            | Event::CirculateNotify(_)
            | Event::GravityNotify(_) => lock(pending).windows_changed(),
            Event::ClientMessage(message) if message.window == wake => break,
            _ => {}
        }
    }
    let _ = conn.damage_destroy(damage);
    let _ = conn.xfixes_destroy_region(region);
    let _ = conn.destroy_window(wake);
    let _ = conn.flush();
}

/// Add the damage read from the server; stop when it could not be read or
/// the listener stopped meanwhile. `false` once stopped.
fn collect_damage(pending: &mut PendingChanges, parts: Option<xfixes::FetchRegionReply>) -> bool {
    match parts {
        Some(parts) if !pending.is_stopped() => {
            for rect in &parts.rectangles {
                pending.push(screen_rect(rect));
            }
            true
        }
        _ => {
            pending.stop();
            false
        }
    }
}

fn screen_rect(rect: &Rectangle) -> ScreenRect {
    ScreenRect::from_bounds(&WindowBounds {
        x: i32::from(rect.x),
        y: i32::from(rect.y),
        width: u32::from(rect.width),
        height: u32::from(rect.height),
    })
}

fn lock(pending: &Mutex<PendingChanges>) -> MutexGuard<'_, PendingChanges> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ChangeListener for X11DamageListener {
    fn drain(&self) -> Result<ListenerChanges, CoreError> {
        lock(&self.pending).drain()
    }

    fn stop(&self) {
        if lock(&self.pending).stop() {
            let wake = ClientMessageEvent::new(32, self.wake, AtomEnum::NONE, [0u32; 5]);
            let _ = self
                .conn
                .send_event(false, self.wake, EventMask::NO_EVENT, wake);
            let _ = self.conn.flush();
        }
    }

    fn name(&self) -> &str {
        "x11-damage"
    }
}

impl Drop for X11DamageListener {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
#[path = "x11_tests.rs"]
mod tests;
