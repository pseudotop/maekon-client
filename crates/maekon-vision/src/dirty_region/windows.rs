//! Windows change listener over DXGI Desktop Duplication.
//!
//! One thread duplicates every output attached to the desktop, each on a
//! Direct3D 11 device of the adapter that drives it (a device on another
//! adapter fails on hybrid-GPU machines). Every [`POLL`] it reads each
//! output's dirty rectangles and move destinations without waiting, and runs the
//! out-of-context window event callbacks for
//! [`super::windows_rules::EVENT_RANGES`]; an event about a top-level window
//! marks the interval as `windows_changed`.
//!
//! DXGI loses a duplication when the desktop switches (the lock screen, a
//! UAC prompt), the display mode changes, or a full-screen exclusive app
//! starts. The listener cannot follow the screen then, so it reports
//! `windows_changed` and duplicates the outputs again every [`RETRY`] until
//! that succeeds.
//!
//! The thread checks its capture permit before handing over what a poll
//! read, and stops on the first refusal; [`ChangeListener::stop`] ends it
//! within one poll.
//!
//! This file compiles only for Windows, so the mutation gates leave it out
//! (`.cargo/mutants.toml`) and no PR job runs it. It has been checked by a
//! cross-compile, not run on a device (#13655). Its arithmetic and event
//! filter live in the measured [`super::windows_rules`], its hand-over in
//! [`PendingChanges::hand_over`].

use std::cell::Cell;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use maekon_core::error::CoreError;
use maekon_core::ports::vision::{CaptureAccess, CapturePermit};
use windows::core::{Interface, Result as WinResult};
use windows::Win32::Foundation::{E_FAIL, HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication,
    IDXGIResource, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    DXGI_OUTDUPL_MOVE_RECT,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetAncestor, PeekMessageW, GA_ROOT, MSG, PM_REMOVE, WINEVENT_OUTOFCONTEXT,
};

use super::mask::ScreenRect;
use super::pending::PendingChanges;
use super::windows_rules::{
    is_layout_event, is_top_level_or_gone, on_desktop, DuplicatedImage, EVENT_RANGES,
};
use super::{ChangeListener, ListenerChanges};

/// How often the thread reads the outputs and the window events.
const POLL: Duration = Duration::from_millis(33);

/// How often the thread duplicates the outputs again after DXGI lost them.
const RETRY: Duration = Duration::from_secs(1);

/// The DXGI listener under `permit`, or `None` where it cannot start.
/// Starting creates Direct3D devices, so call this from a blocking context.
pub fn session_listener(permit: CapturePermit) -> Option<Box<dyn ChangeListener>> {
    DxgiDuplicationListener::start(permit).map(|listener| Box::new(listener) as _)
}

/// [`ChangeListener`] over DXGI Desktop Duplication of every output.
pub struct DxgiDuplicationListener {
    pending: Arc<Mutex<PendingChanges>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl DxgiDuplicationListener {
    /// Listen under `permit`. `None` when an output attached to the desktop
    /// cannot be duplicated or the window events cannot be hooked: a
    /// listener that missed one output would miss its changes.
    pub fn start(permit: CapturePermit) -> Option<Self> {
        let pending = Arc::new(Mutex::new(PendingChanges::default()));
        let (ready, started) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("maekon-dxgi-duplication".to_string())
            .spawn({
                let pending = pending.clone();
                // The hooks must be removed by the thread that installed them,
                // and the DXGI objects stay on the thread that uses them.
                move || {
                    let setup = WindowEventHooks::install().zip(duplicate_outputs().ok());
                    let _ = ready.send(setup.is_some());
                    if let Some((hooks, outputs)) = setup {
                        listen(&pending, &permit, outputs);
                        drop(hooks);
                    }
                }
            })
            .ok()?;
        if started.recv() != Ok(true) {
            let _ = thread.join();
            return None;
        }
        Some(Self {
            pending,
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

/// Poll until stopped or the permit is refused.
fn listen(pending: &Mutex<PendingChanges>, permit: &CapturePermit, outputs: Vec<Output>) {
    let mut duplication = Duplication::Running(outputs);
    loop {
        pump_window_events();
        let mut rects = Vec::new();
        let followed = duplication.read(&mut rects);
        let windows_changed = LAYOUT_CHANGED.replace(false) || !followed;
        let allowed = permit.check(CaptureAccess::Retain).is_ok();
        if !lock(pending).hand_over(allowed, rects, windows_changed) {
            break;
        }
        std::thread::sleep(POLL);
    }
}

fn lock(pending: &Mutex<PendingChanges>) -> MutexGuard<'_, PendingChanges> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ChangeListener for DxgiDuplicationListener {
    fn drain(&self) -> Result<ListenerChanges, CoreError> {
        lock(&self.pending).drain()
    }

    fn stop(&self) {
        lock(&self.pending).stop();
    }

    fn name(&self) -> &str {
        "dxgi-duplication"
    }
}

impl Drop for DxgiDuplicationListener {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The duplicated outputs, or when to try again after DXGI lost them.
enum Duplication {
    Running(Vec<Output>),
    Lost { retry_at: Instant },
}

impl Duplication {
    /// Add what changed on every output to `rects`. `false` when the screen
    /// could not be followed during this poll.
    fn read(&mut self, rects: &mut Vec<ScreenRect>) -> bool {
        match self {
            Self::Running(outputs) => {
                let Err(error) = outputs.iter().try_for_each(|output| output.read(rects)) else {
                    return true;
                };
                tracing::debug!(%error, "DXGI duplication lost; duplicating the outputs again");
                *self = Self::Lost {
                    retry_at: Instant::now(),
                };
                false
            }
            Self::Lost { retry_at } => {
                if Instant::now() >= *retry_at {
                    *self = duplicate_outputs().map_or_else(
                        |_| Self::Lost {
                            retry_at: Instant::now() + RETRY,
                        },
                        Self::Running,
                    );
                }
                false
            }
        }
    }
}

/// One duplicated output and where it lies on the virtual screen.
struct Output {
    duplication: IDXGIOutputDuplication,
    image: DuplicatedImage,
    desktop: ScreenRect,
    /// The device the duplication was created on, kept alive with it.
    _device: ID3D11Device,
}

impl Output {
    /// Add what changed since the previous read to `rects`, without waiting.
    fn read(&self, rects: &mut Vec<ScreenRect>) -> WinResult<()> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: both out-pointers point to live locals for the whole call.
        let acquired = unsafe {
            self.duplication
                .AcquireNextFrame(0, &mut info, &mut resource)
        };
        match acquired {
            Ok(()) => {}
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(()),
            Err(error) => return Err(error),
        }
        drop(resource);
        let read = self.frame_rects(info.TotalMetadataBufferSize, rects);
        // SAFETY: a frame was acquired above, and it is released once.
        let released = unsafe { self.duplication.ReleaseFrame() };
        read.and(released)
    }

    /// The acquired frame's move destinations and dirty rectangles; `bytes`
    /// is the size of both (`TotalMetadataBufferSize`), 0 when only the
    /// pointer moved.
    fn frame_rects(&self, bytes: u32, rects: &mut Vec<ScreenRect>) -> WinResult<()> {
        if bytes == 0 {
            return Ok(());
        }
        let moves = metadata::<DXGI_OUTDUPL_MOVE_RECT>(bytes, |size, buffer, used| {
            // SAFETY: `buffer` holds at least `size` bytes and `used` is live.
            unsafe { self.duplication.GetFrameMoveRects(size, buffer, used) }
        })?;
        let dirty = metadata::<RECT>(bytes, |size, buffer, used| {
            // SAFETY: as above.
            unsafe { self.duplication.GetFrameDirtyRects(size, buffer, used) }
        })?;
        let changed = moves
            .iter()
            .map(|change| &change.DestinationRect)
            .chain(&dirty)
            .map(|rect| on_desktop(screen_rect(rect), &self.image, &self.desktop));
        rects.extend(changed);
        Ok(())
    }
}

/// Read one kind of frame metadata into a buffer of at least `bytes`.
fn metadata<T: Default + Clone>(
    bytes: u32,
    read: impl FnOnce(u32, *mut T, *mut u32) -> WinResult<()>,
) -> WinResult<Vec<T>> {
    let size = std::mem::size_of::<T>();
    let mut buffer = vec![T::default(); (bytes as usize).div_ceil(size)];
    let mut used = 0u32;
    read(bytes, buffer.as_mut_ptr(), &mut used)?;
    buffer.truncate(used as usize / size);
    Ok(buffer)
}

fn screen_rect(rect: &RECT) -> ScreenRect {
    ScreenRect {
        left: i64::from(rect.left),
        top: i64::from(rect.top),
        right: i64::from(rect.right),
        bottom: i64::from(rect.bottom),
    }
}

/// Duplicate every output attached to the desktop; an error when any of
/// them cannot be duplicated, or there is none.
fn duplicate_outputs() -> WinResult<Vec<Output>> {
    // SAFETY: no arguments; the factory is a counted COM reference.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }?;
    // SAFETY: enumeration by index; DXGI_ERROR_NOT_FOUND ends it.
    let adapters = enumerate(|index| unsafe { factory.EnumAdapters1(index) })?;
    let mut outputs = Vec::new();
    for adapter in adapters {
        let mut attached = Vec::new();
        // SAFETY: as above.
        for output in enumerate(|index| unsafe { adapter.EnumOutputs(index) })? {
            // SAFETY: GetDesc only fills in the returned description.
            let desc = unsafe { output.GetDesc() }?;
            if desc.AttachedToDesktop.as_bool() {
                attached.push((output.cast::<IDXGIOutput1>()?, desc));
            }
        }
        if attached.is_empty() {
            continue;
        }
        let device = create_device(&adapter)?;
        for (output, desc) in attached {
            // SAFETY: the device lives as long as the duplication (`Output`).
            let duplication = unsafe { output.DuplicateOutput(&device) }?;
            // SAFETY: GetDesc only fills in the returned description.
            let mode = unsafe { duplication.GetDesc() };
            outputs.push(Output {
                image: DuplicatedImage {
                    width: i64::from(mode.ModeDesc.Width),
                    height: i64::from(mode.ModeDesc.Height),
                    rotation: mode.Rotation.0,
                },
                duplication,
                desktop: screen_rect(&desc.DesktopCoordinates),
                _device: device.clone(),
            });
        }
    }
    if outputs.is_empty() {
        return Err(DXGI_ERROR_NOT_FOUND.into());
    }
    Ok(outputs)
}

/// Items `0, 1, ...` of a DXGI enumeration, up to `DXGI_ERROR_NOT_FOUND`.
fn enumerate<T>(mut item: impl FnMut(u32) -> WinResult<T>) -> WinResult<Vec<T>> {
    let mut items = Vec::new();
    for index in 0u32.. {
        match item(index) {
            Ok(found) => items.push(found),
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(error),
        }
    }
    Ok(items)
}

/// A Direct3D 11 device on `adapter`, for duplicating its outputs.
fn create_device(adapter: &IDXGIAdapter1) -> WinResult<ID3D11Device> {
    let mut device = None;
    // SAFETY: `device` outlives the call; no feature level or context is
    // requested, and an explicit adapter needs D3D_DRIVER_TYPE_UNKNOWN.
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    }?;
    device.ok_or_else(|| E_FAIL.into())
}

thread_local! {
    /// Set by [`on_window_event`] on the listener thread, read by its loop.
    static LAYOUT_CHANGED: Cell<bool> = const { Cell::new(false) };
}

/// The listener thread's out-of-context window event hooks, removed on drop.
struct WindowEventHooks(Vec<HWINEVENTHOOK>);

impl WindowEventHooks {
    fn install() -> Option<Self> {
        let mut hooks = Self(Vec::new());
        for (first, last) in EVENT_RANGES {
            // SAFETY: `on_window_event` has the WINEVENTPROC signature and
            // lives for the whole program; an out-of-context hook needs no
            // module, and process and thread 0 mean every one.
            let hook = unsafe {
                SetWinEventHook(
                    first,
                    last,
                    None,
                    Some(on_window_event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            if hook.is_invalid() {
                return None;
            }
            hooks.0.push(hook);
        }
        Some(hooks)
    }
}

impl Drop for WindowEventHooks {
    fn drop(&mut self) {
        for hook in self.0.drain(..) {
            // SAFETY: this thread installed the hook, and it is removed once.
            let _ = unsafe { UnhookWinEvent(hook) };
        }
    }
}

/// Runs on the listener thread while it pumps messages.
extern "system" fn on_window_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if !is_layout_event(event, object, child) {
        return;
    }
    // SAFETY: GetAncestor takes any handle and returns null for one that is
    // gone.
    let root = unsafe { GetAncestor(window, GA_ROOT) };
    if is_top_level_or_gone(window.0 as isize, root.0 as isize) {
        LAYOUT_CHANGED.set(true);
    }
}

/// Run the window event callbacks that arrived since the previous pump.
fn pump_window_events() {
    let mut message = MSG::default();
    // SAFETY: `message` is a live local the call writes into.
    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
        // SAFETY: `message` was just filled in by PeekMessageW.
        unsafe { DispatchMessageW(&message) };
    }
}
