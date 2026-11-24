//! macOS change listener over ScreenCaptureKit (feature
//! `macos-screencapturekit`, off by default; see the feature's note in
//! `Cargo.toml`).
//!
//! One stream per active display, each at the display's pixel size, delivers
//! frames to a serial dispatch queue. The stream output reads what each
//! frame's attachments say changed ([`super::macos_rules::frame_changes`])
//! into an inbox; the frames themselves are not read. Every [`POLL`] the
//! listener thread takes the inbox, reads the window list again when a frame
//! arrived (a window change that drew nothing has nothing to cut out),
//! checks its capture permit and hands over. The stream leaves the cursor
//! out, so pointer movement is no change.
//!
//! A stopped stream, or a change of the active displays, means the listener
//! did not follow the screen: it reports `windows_changed` and starts the
//! streams again every [`RETRY`] until that succeeds. Streams need the
//! Screen Recording permission, and macOS may show its screen recording
//! indicator while they run.
//!
//! This file compiles only for macOS, so the mutation gates leave it out
//! (`.cargo/mutants.toml`) and no PR job runs it. It has been checked by a
//! cross-compile, not run on a device (#13655). Its frame and window-list
//! rules live in the measured [`super::macos_rules`], its hand-over in
//! [`PendingChanges::hand_over`].

use std::collections::BTreeSet;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use block2::{DynBlock, RcBlock};
use dispatch2::{DispatchQueue, DispatchRetained};
use maekon_core::error::CoreError;
use maekon_core::ports::vision::{CaptureAccess, CapturePermit};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_foundation::{CFArray, CFDictionary, CGRect};
use objc2_core_graphics::{
    kCGNullWindowID, kCGWindowBounds, kCGWindowNumber, CGDirectDisplayID, CGDisplayBounds,
    CGDisplayCopyDisplayMode, CGDisplayMode, CGError, CGGetActiveDisplayList,
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString, NSValue};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate, SCStreamFrameInfoContentRect, SCStreamFrameInfoContentScale,
    SCStreamFrameInfoDirtyRects, SCStreamFrameInfoScaleFactor, SCStreamFrameInfoStatus,
    SCStreamOutput, SCStreamOutputType, SCWindow,
};

use super::macos_rules::{frame_changes, layout_changed, FrameInfo, FrameRect, Inbox, WindowEntry};
use super::mask::ScreenRect;
use super::pending::PendingChanges;
use super::{ChangeListener, ListenerChanges};

/// How often the thread takes what the streams delivered.
const POLL: Duration = Duration::from_millis(33);

/// How often the thread starts the streams again after one stopped.
const RETRY: Duration = Duration::from_secs(1);

/// How long to wait for ScreenCaptureKit to answer a request.
const ANSWER: Duration = Duration::from_secs(5);

/// The most frames per second a stream delivers, about one per poll.
const FRAMES_PER_SECOND: i32 = 30;

/// `'420v'`, the smallest pixel format a stream offers; frames are not read.
const PIXEL_FORMAT: u32 = u32::from_be_bytes(*b"420v");

/// Frames a stream keeps queued; the output returns at once, so few are
/// needed (ScreenCaptureKit's default is 8).
const QUEUE_DEPTH: isize = 3;

/// The most active displays the listener reads.
const MAX_DISPLAYS: u32 = 32;

/// The ScreenCaptureKit listener under `permit`, or `None` where it cannot
/// start. Starting waits for ScreenCaptureKit, so call this from a blocking
/// context.
pub fn session_listener(permit: CapturePermit) -> Option<Box<dyn ChangeListener>> {
    ScreenCaptureKitListener::start(permit).map(|listener| Box::new(listener) as _)
}

/// [`ChangeListener`] over a ScreenCaptureKit stream of every active display.
pub struct ScreenCaptureKitListener {
    pending: Arc<Mutex<PendingChanges>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ScreenCaptureKitListener {
    /// Listen under `permit`. `None` without the Screen Recording permission,
    /// when an active display cannot be streamed, or when ScreenCaptureKit
    /// does not answer: a listener that missed one display would miss its
    /// changes.
    pub fn start(permit: CapturePermit) -> Option<Self> {
        let pending = Arc::new(Mutex::new(PendingChanges::default()));
        let (ready, started) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("maekon-screencapturekit".to_string())
            .spawn({
                let pending = pending.clone();
                // The streams are started, read and stopped on this thread.
                move || {
                    let session = Session::start();
                    let _ = ready.send(session.is_some());
                    if let Some(session) = session {
                        listen(&pending, &permit, session);
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
        lock(&self.thread)
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }
}

/// Poll until stopped or the permit is refused.
fn listen(pending: &Mutex<PendingChanges>, permit: &CapturePermit, session: Session) {
    let mut capture = Capture::Running(session);
    let mut layout = window_layout();
    loop {
        let inbox = capture.take();
        let mut windows_changed = inbox.not_followed || inbox.stopped;
        if inbox.frames {
            let now = window_layout();
            windows_changed |= layout_changed(layout.as_ref(), now.as_ref());
            layout = now;
        }
        let allowed = permit.check(CaptureAccess::Retain).is_ok();
        if !lock(pending).hand_over(allowed, inbox.rects, windows_changed) {
            break;
        }
        std::thread::sleep(POLL);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ChangeListener for ScreenCaptureKitListener {
    fn drain(&self) -> Result<ListenerChanges, CoreError> {
        lock(&self.pending).drain()
    }

    fn stop(&self) {
        lock(&self.pending).stop();
    }

    fn name(&self) -> &str {
        "screencapturekit"
    }
}

impl Drop for ScreenCaptureKitListener {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The running streams, or when to start them again after they were lost.
enum Capture {
    Running(Session),
    Lost { retry_at: Instant },
}

impl Capture {
    /// What the streams delivered since the previous take. The session is
    /// lost when a stream stopped or the active displays changed, and starts
    /// again once due; until then every take reports `stopped`.
    fn take(&mut self) -> Inbox {
        match self {
            Self::Running(session) => {
                let mut inbox = std::mem::take(&mut *lock(&session.inbox));
                if inbox.stopped || active_displays().as_ref() != Some(&session.displays) {
                    tracing::debug!("ScreenCaptureKit streams lost; starting them again");
                    inbox.stopped = true;
                    *self = Self::Lost {
                        retry_at: Instant::now(),
                    };
                }
                inbox
            }
            Self::Lost { retry_at } => {
                if Instant::now() >= *retry_at {
                    *self = Session::start().map_or_else(
                        || Self::Lost {
                            retry_at: Instant::now() + RETRY,
                        },
                        Self::Running,
                    );
                }
                Inbox {
                    stopped: true,
                    ..Inbox::default()
                }
            }
        }
    }
}

/// An active display and its area in global display coordinates (points).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisplayArea {
    id: CGDirectDisplayID,
    bounds: ScreenRect,
}

/// The active displays, as CoreGraphics lists them; `None` when they cannot
/// be read.
fn active_displays() -> Option<Vec<DisplayArea>> {
    let mut ids = [0; MAX_DISPLAYS as usize];
    let mut count = 0u32;
    // SAFETY: both pointers point to live locals, and `ids` holds
    // MAX_DISPLAYS entries.
    let error = unsafe { CGGetActiveDisplayList(MAX_DISPLAYS, ids.as_mut_ptr(), &mut count) };
    if error != CGError::Success || count == 0 {
        return None;
    }
    let areas = ids.iter().take(count as usize).map(|&id| DisplayArea {
        id,
        bounds: points(CGDisplayBounds(id)),
    });
    Some(areas.collect())
}

/// A rectangle in points, rounded outwards.
fn points(rect: CGRect) -> ScreenRect {
    let (min, max) = (rect.min(), rect.max());
    ScreenRect {
        left: min.x.floor() as i64,
        top: min.y.floor() as i64,
        right: max.x.ceil() as i64,
        bottom: max.y.ceil() as i64,
    }
}

/// The streams of every active display, started together.
struct Session {
    /// The active displays when the session started.
    displays: Vec<DisplayArea>,
    inbox: Arc<Mutex<Inbox>>,
    streams: Vec<Stream>,
    /// The serial queue every stream output runs on.
    queue: DispatchRetained<DispatchQueue>,
}

impl Session {
    /// `None` when an active display cannot be streamed. Streams already
    /// started are stopped again when the half-built session drops.
    fn start() -> Option<Self> {
        let displays = active_displays()?;
        let content = shareable_content()?;
        // SAFETY: a property of an immutable snapshot.
        let shareable = unsafe { content.displays() };
        let mut session = Self {
            displays: Vec::new(),
            inbox: Arc::new(Mutex::new(Inbox::default())),
            streams: Vec::new(),
            queue: DispatchQueue::new("maekon.dirty-region.screencapturekit", None),
        };
        for area in &displays {
            // SAFETY: a property of an immutable snapshot.
            let display = shareable
                .iter()
                .find(|display| unsafe { display.displayID() } == area.id)?;
            let stream = Stream::start(&display, area, &session.inbox, &session.queue)?;
            session.streams.push(stream);
        }
        session.displays = displays;
        Some(session)
    }
}

impl Drop for Session {
    /// Stop every stream, then wait for the outputs already queued, so no
    /// output runs once the session is gone. A stream that does not confirm
    /// its stop may still deliver frames, so its session's streams are kept
    /// alive for good.
    fn drop(&mut self) {
        let mut stopped = true;
        for stream in &self.streams {
            // Not `&&`: every stream is stopped, whatever the others did.
            stopped &= stream.stop();
        }
        if stopped {
            self.queue.exec_sync(|| {});
        } else {
            self.streams.drain(..).for_each(std::mem::forget);
        }
    }
}

/// One display's stream and the output it delivers to.
struct Stream {
    stream: Retained<SCStream>,
    /// The stream's output and delegate, kept alive with it.
    _output: Retained<StreamOutput>,
}

impl Stream {
    /// Stream `display` at its pixel size into `inbox`, on `queue`.
    fn start(
        display: &SCDisplay,
        area: &DisplayArea,
        inbox: &Arc<Mutex<Inbox>>,
        queue: &DispatchQueue,
    ) -> Option<Self> {
        let mode = CGDisplayCopyDisplayMode(area.id)?;
        let width = CGDisplayMode::pixel_width(Some(&mode));
        let height = CGDisplayMode::pixel_height(Some(&mode));
        let output = StreamOutput::new(area.bounds, inbox.clone());
        // SAFETY: creating and configuring objects this function owns; the
        // filter, configuration and delegate are valid for the call.
        let stream = unsafe {
            let filter = SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                display,
                &NSArray::<SCWindow>::new(),
            );
            let config = SCStreamConfiguration::new();
            config.setWidth(width);
            config.setHeight(height);
            config.setScalesToFit(false);
            config.setShowsCursor(false);
            config.setPixelFormat(PIXEL_FORMAT);
            config.setQueueDepth(QUEUE_DEPTH);
            config.setMinimumFrameInterval(CMTime::new(1, FRAMES_PER_SECOND));
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*output)),
            )
        };
        // SAFETY: `queue` is serial and outlives the stream (`Session`).
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                Some(queue),
            )
        }
        .ok()?;
        let this = Self {
            stream,
            _output: output,
        };
        // SAFETY: the handler has the declared block signature.
        let started =
            answer(|done| unsafe { this.stream.startCaptureWithCompletionHandler(Some(done)) });
        match started {
            Some(true) => Some(this),
            // It never started, so nothing calls the output.
            Some(false) => None,
            // It may still start and call the output.
            None => {
                std::mem::forget(this);
                None
            }
        }
    }

    /// Stop the stream; `true` once ScreenCaptureKit confirmed it, with or
    /// without an error.
    fn stop(&self) -> bool {
        // SAFETY: the handler has the declared block signature.
        answer(|done| unsafe { self.stream.stopCaptureWithCompletionHandler(Some(done)) }).is_some()
    }
}

/// Whether ScreenCaptureKit finished what `call` asked without an error;
/// `None` when it did not answer within [`ANSWER`].
fn answer(call: impl FnOnce(&DynBlock<dyn Fn(*mut NSError)>)) -> Option<bool> {
    let (finished, answered) = mpsc::channel();
    let handler = RcBlock::new(move |error: *mut NSError| {
        let _ = finished.send(error.is_null());
    });
    call(&handler);
    answered.recv_timeout(ANSWER).ok()
}

/// What ScreenCaptureKit may capture now; `None` without the Screen
/// Recording permission, or when it did not answer within [`ANSWER`].
fn shareable_content() -> Option<Retained<SCShareableContent>> {
    let (delivered, received) = mpsc::channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, _error: *mut NSError| {
            // SAFETY: a non-null content is valid for the call; retaining it
            // keeps it valid afterwards.
            let _ = delivered.send(Delivered(unsafe { Retained::retain(content) }));
        },
    );
    // SAFETY: the handler has the declared block signature.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &handler,
        );
    }
    received.recv_timeout(ANSWER).ok()?.0
}

/// Shareable content handed from the queue ScreenCaptureKit answered on to
/// the listener thread.
struct Delivered(Option<Retained<SCShareableContent>>);

// SAFETY: the content is an immutable snapshot that ScreenCaptureKit itself
// creates on one queue and hands to another, and only the receiving thread
// uses it.
unsafe impl Send for Delivered {}

/// What a stream output needs: its display's area in global display
/// coordinates, and where to put what frames say.
struct OutputState {
    display: ScreenRect,
    inbox: Arc<Mutex<Inbox>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "MaekonDirtyRegionStreamOutput"]
    #[ivars = OutputState]
    struct StreamOutput;

    // SAFETY: an NSObject subclass conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for StreamOutput {}

    // SAFETY: the method has the protocol's signature, and it neither
    // panics nor unwinds.
    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            let state = self.ivars();
            let changes = frame_changes(&frame_info(sample), &state.display);
            lock(&state.inbox).receive(changes);
        }
    }

    // SAFETY: as above.
    unsafe impl SCStreamDelegate for StreamOutput {
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_did_stop(&self, _stream: &SCStream, _error: &NSError) {
            lock(&self.ivars().inbox).stopped = true;
        }
    }
);

impl StreamOutput {
    fn new(display: ScreenRect, inbox: Arc<Mutex<Inbox>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputState { display, inbox });
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// The attachments of a frame's sample (`SCStreamFrameInfo` keys).
fn frame_info(sample: &CMSampleBuffer) -> FrameInfo {
    // SAFETY: the sample is valid for the callback; not creating the
    // attachments leaves it unchanged.
    let Some(attachments) = (unsafe { sample.sample_attachments_array(false) }) else {
        return FrameInfo::default();
    };
    // SAFETY: CFArray is toll-free bridged to NSArray.
    let attachments = unsafe { &*(&*attachments as *const CFArray).cast::<NSArray>() };
    let Some(info) = attachments
        .firstObject()
        .and_then(|info| info.downcast::<NSDictionary>().ok())
    else {
        return FrameInfo::default();
    };
    // SAFETY: the keys are framework constants.
    let (status, scale_factor, content_scale, content_rect, dirty_rects) = unsafe {
        (
            SCStreamFrameInfoStatus,
            SCStreamFrameInfoScaleFactor,
            SCStreamFrameInfoContentScale,
            SCStreamFrameInfoContentRect,
            SCStreamFrameInfoDirtyRects,
        )
    };
    let number = |key: &NSString| {
        info.objectForKey(key)
            .and_then(|value| value.downcast::<NSNumber>().ok())
    };
    FrameInfo {
        status: number(status).map(|status| status.as_i64()),
        scale_factor: number(scale_factor).map(|scale| scale.as_f64()),
        content_scale: number(content_scale).map(|scale| scale.as_f64()),
        content_rect: info
            .objectForKey(content_rect)
            .and_then(|rect| frame_rect(&rect)),
        dirty_rects: info
            .objectForKey(dirty_rects)
            .and_then(|rects| rects.downcast::<NSArray>().ok())
            .map(|rects| rects.iter().map(|rect| frame_rect(&rect)).collect()),
    }
}

/// A rectangle stored as an `NSValue` of a `CGRect`, or as the dictionary
/// `CGRectCreateDictionaryRepresentation` makes.
fn frame_rect(value: &AnyObject) -> Option<FrameRect> {
    let rect = match value.downcast_ref::<NSValue>() {
        Some(value) => value.get_rect()?,
        None => {
            let dictionary = value.downcast_ref::<NSDictionary>()?;
            let mut rect = CGRect::default();
            // SAFETY: NSDictionary is toll-free bridged to CFDictionary, and
            // `rect` is a live local the call writes into.
            let read = unsafe {
                CGRectMakeWithDictionaryRepresentation(
                    Some(&*(dictionary as *const NSDictionary).cast::<CFDictionary>()),
                    &mut rect,
                )
            };
            read.then_some(rect)?
        }
    };
    Some(FrameRect {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    })
}

/// The on-screen windows; `None` when the window list cannot be read.
fn window_layout() -> Option<BTreeSet<WindowEntry>> {
    let windows = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    )?;
    // SAFETY: CFArray is toll-free bridged to NSArray.
    let windows = unsafe { &*(&*windows as *const CFArray).cast::<NSArray>() };
    // SAFETY: the keys are framework constants.
    let (number, bounds) = unsafe { (kCGWindowNumber, kCGWindowBounds) };
    let (number, bounds): (&NSString, &NSString) = (number.as_ref(), bounds.as_ref());
    windows
        .iter()
        .map(|window| {
            let window = window.downcast::<NSDictionary>().ok()?;
            let id = window.objectForKey(number)?.downcast::<NSNumber>().ok()?;
            let area = window.objectForKey(bounds)?;
            let area = frame_rect(&area)?;
            let area = [area.x, area.y, area.width, area.height].map(f64::to_bits);
            Some((id.as_i64(), area))
        })
        .collect()
}
