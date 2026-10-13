use crate::encode::{stream::STREAM_FPS, PixelFormat, RawFrame};
use protocol::control::MonitorInfo;
use scap::capturer::engine::{ChannelItem, Engine};
use scap::capturer::Options;
use scap::frame::{Frame, FrameType};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
mod secure_desktop;

// Scap 0.0.8 supplies the platform engines here: ScreenCaptureKit on macOS,
// Windows.Graphics.Capture on Windows, and PipeWire/portal on Linux. The
// Windows and Linux branches are compile-tested only on their respective CI
// runners; this macOS build verifies the unchanged macOS engine.

#[cfg(target_os = "macos")]
fn capture_permission_available() -> bool {
    scap::has_permission() || scap::request_permission()
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn capture_permission_available() -> bool {
    // Scap reports these platforms as available and performs any platform
    // interaction while building/starting its capturer.
    true
}

/// A source of screen frames for the host video pipeline.
///
/// `next_frame` must not block: it returns the newest frame captured since
/// the previous call, or `None` when nothing new arrived (a static screen),
/// in which case the caller repeats the last frame it has.
pub trait ScreenCapturer {
    fn next_frame(&mut self) -> Option<RawFrame>;

    /// Like `next_frame`, but waits up to `timeout` for a frame to arrive so
    /// the caller can encode as soon as the screen changes. The default
    /// polls once more after sleeping; capturers with a frame-arrival signal
    /// override it.
    fn wait_frame(&mut self, timeout: Duration) -> Option<RawFrame> {
        if let Some(frame) = self.next_frame() {
            return Some(frame);
        }
        std::thread::sleep(timeout);
        self.next_frame()
    }

    fn monitors(&self) -> Vec<MonitorInfo> { Vec::new() }

    fn select_monitor(&mut self, _id: u32) -> bool { false }

    /// Since when capture has been down and is being restarted; `None`
    /// while it runs.
    fn stalled_since(&self) -> Option<Instant> { None }

    /// The displays, when they changed since the last call.
    fn monitors_changed(&mut self) -> Option<Vec<MonitorInfo>> { None }
}

impl<T: ScreenCapturer + ?Sized> ScreenCapturer for Box<T> {
    fn next_frame(&mut self) -> Option<RawFrame> { (**self).next_frame() }
    fn wait_frame(&mut self, timeout: Duration) -> Option<RawFrame> { (**self).wait_frame(timeout) }
    fn monitors(&self) -> Vec<MonitorInfo> { (**self).monitors() }
    fn select_monitor(&mut self, id: u32) -> bool { (**self).select_monitor(id) }
    fn stalled_since(&self) -> Option<Instant> { (**self).stalled_since() }
    fn monitors_changed(&mut self) -> Option<Vec<MonitorInfo>> { (**self).monitors_changed() }
}

/// How long the capture thread waits for a frame before it checks for
/// monitor switches and shutdown.
const CAPTURE_POLL: Duration = Duration::from_millis(20);
/// How often the capture thread looks for added or removed displays.
const MONITOR_CHECK: Duration = Duration::from_secs(2);
/// First wait before restarting a stopped capture; doubles per failed
/// attempt up to [`MAX_RESTART_DELAY`].
const FIRST_RESTART_DELAY: Duration = Duration::from_millis(250);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(2);
/// A restarted capture counts as working once it delivers a frame, or runs
/// this long without failing (a static screen may send nothing).
const RESTART_CONFIRM: Duration = Duration::from_secs(2);

/// Newest frame published by the capture thread, tagged with the monitor
/// generation it was captured for.
#[derive(Default)]
struct LatestFrame {
    slot: Mutex<Option<(u64, RawFrame)>>,
    /// Signalled whenever a frame is published.
    arrived: Condvar,
    /// Bumped by every requested monitor switch; frames tagged with an older
    /// generation come from the previous monitor and are discarded.
    generation: AtomicU64,
    /// See [`ScreenCapturer::stalled_since`].
    stalled: Mutex<Option<Instant>>,
    /// A new display list the session has not taken yet.
    monitors: Mutex<Option<Vec<MonitorInfo>>>,
}

impl LatestFrame {
    fn publish(&self, generation: u64, frame: RawFrame) {
        *self.slot.lock().unwrap_or_else(|poison| poison.into_inner()) = Some((generation, frame));
        self.arrived.notify_all();
    }
}

enum CaptureCommand {
    SelectMonitor { id: u32, generation: u64 },
}

/// Screen capture through scap, on a dedicated OS thread.
///
/// Scap's `get_next_frame` blocks until the platform delivers a frame, and
/// Windows Graphics Capture and PipeWire only deliver frames when the screen
/// changes, so calling it from the session's async runtime would stall input,
/// audio and control on a static screen. Scap's Windows channel is also
/// unbounded and ignores the frame rate. The capture thread therefore owns
/// the scap engine, drains its channel continuously and keeps only the newest
/// frame, which `next_frame` takes without blocking. Monitor switches run on
/// that thread too. Dropping the capturer stops capture and joins the thread.
pub struct ScapCapturer {
    latest: Arc<LatestFrame>,
    commands: Option<std_mpsc::Sender<CaptureCommand>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ScapCapturer {
    pub fn create() -> Option<Box<dyn ScreenCapturer + Send>> {
        #[cfg(target_os = "windows")]
        if crate::secure_attention::available() {
            return secure_desktop::SecureDesktopCapturer::new()
                .map(|capturer| Box::new(capturer) as Box<dyn ScreenCapturer + Send>);
        }
        if !scap::is_supported() {
            return None;
        }

        if !capture_permission_available() {
            return None;
        }

        let latest = Arc::new(LatestFrame::default());
        let (commands, command_rx) = std_mpsc::channel();
        let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);
        let shared = Arc::clone(&latest);
        let worker = std::thread::Builder::new()
            .name("opendesk-capture".into())
            .spawn(move || {
                let mut platform = Scap;
                let Some(stream) = platform.start(None) else {
                    let _ = ready_tx.send(false);
                    return;
                };
                let _ = ready_tx.send(true);
                capture_loop(platform, stream, &command_rx, &shared, MONITOR_CHECK);
            })
            .ok()?;
        if !ready_rx.recv().unwrap_or(false) {
            let _ = worker.join();
            return None;
        }
        Some(Box::new(Self { latest, commands: Some(commands), worker: Some(worker) }))
    }
}

/// Capture size requested from the platform. ScreenCaptureKit scales on the
/// GPU, so macOS asks for a 1080p-class stream (width capped at 1920) instead
/// of the native Retina size. Scap ignores this setting on Windows and Linux;
/// the video pipeline scales those captures in software.
#[cfg(target_os = "macos")]
fn output_resolution() -> scap::capturer::Resolution {
    scap::capturer::Resolution::_1080p
}

#[cfg(not(target_os = "macos"))]
fn output_resolution() -> scap::capturer::Resolution {
    scap::capturer::Resolution::Captured
}

/// A running scap engine and the channel it delivers frames on.
///
/// The engine is driven directly instead of through
/// `scap::capturer::Capturer` because the capturer's channel has no receive
/// timeout, so a capture thread blocked in it could never be stopped.
struct CaptureStream {
    engine: Engine,
    rx: std_mpsc::Receiver<ChannelItem>,
}

impl CaptureStream {
    /// Builds and starts an engine for `target` (the main display if
    /// `None`). Scap panics on some engine failures; those are caught.
    fn start(target: Option<scap::Target>) -> Option<Self> {
        let options = Options {
            fps: STREAM_FPS,
            target,
            output_type: FrameType::BGRAFrame,
            output_resolution: output_resolution(),
            ..Default::default()
        };
        let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let (tx, rx) = std_mpsc::channel();
            let mut engine = Engine::new(&options, tx);
            engine.start();
            Self { engine, rx }
        }));
        if started.is_err() {
            log::warn!("screen capture engine failed to start");
        }
        started.ok()
    }

    /// Starts capturing display `id`.
    fn start_display(id: u32) -> Option<Self> {
        let target = scap::get_all_targets().into_iter().find_map(|target| match target {
            scap::Target::Display(display) if display.id == id => {
                Some(scap::Target::Display(display))
            }
            _ => None,
        });
        let stream = target.and_then(|target| Self::start(Some(target)));
        if stream.is_none() {
            log::warn!("could not switch screen capture to monitor {id}");
        }
        stream
    }

    /// Waits up to `timeout` for frames and returns the newest usable one,
    /// discarding the backlog. `Err` means the engine stopped delivering.
    fn newest_frame(&self, timeout: Duration) -> Result<Option<RawFrame>, ()> {
        let first = match self.rx.recv_timeout(timeout) {
            Ok(item) => item,
            Err(std_mpsc::RecvTimeoutError::Timeout) => return Ok(None),
            Err(std_mpsc::RecvTimeoutError::Disconnected) => return Err(()),
        };
        let mut items = vec![first];
        items.extend(self.rx.try_iter());
        // Newest first; an older item is only converted when every newer one
        // is unusable (macOS reports idle intervals as empty frames).
        for item in items.into_iter().rev() {
            let frame = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.engine.process_channel_item(item)
            }));
            if let Some(frame) = frame.ok().flatten().and_then(frame_to_bgra) {
                return Ok(Some(frame));
            }
        }
        Ok(None)
    }
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        // Stop before the receiver drops: scap's Windows engine panics when it
        // cannot send a frame.
        let engine = &mut self.engine;
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine.stop())).is_err() {
            log::warn!("screen capture engine failed to stop cleanly");
        }
    }
}

/// The platform side of the capture thread, faked in tests.
trait CapturePlatform {
    type Stream: FrameSource;

    /// Starts capturing `display`, or the main display if `None`.
    fn start(&mut self, display: Option<u32>) -> Option<Self::Stream>;

    fn monitors(&mut self) -> Vec<MonitorInfo>;
}

trait FrameSource {
    /// Waits up to `timeout` for the newest frame. `Err` means the stream
    /// stopped for good.
    fn newest_frame(&self, timeout: Duration) -> Result<Option<RawFrame>, ()>;
}

struct Scap;

impl CapturePlatform for Scap {
    type Stream = CaptureStream;

    fn start(&mut self, display: Option<u32>) -> Option<CaptureStream> {
        match display {
            Some(id) => CaptureStream::start_display(id),
            None => CaptureStream::start(None),
        }
    }

    fn monitors(&mut self) -> Vec<MonitorInfo> { list_monitors() }
}

impl FrameSource for CaptureStream {
    fn newest_frame(&self, timeout: Duration) -> Result<Option<RawFrame>, ()> { CaptureStream::newest_frame(self, timeout) }
}

/// Restart state after capture stopped: when it stopped, and when to try
/// again.
#[derive(Debug, Default)]
struct Recovery {
    since: Option<Instant>,
    failures: u32,
    next_try: Option<Instant>,
    restarted_at: Option<Instant>,
}

impl Recovery {
    fn lost(&mut self, now: Instant) {
        self.since.get_or_insert(now);
        self.restarted_at = None;
        self.next_try = Some(now + restart_delay(self.failures));
    }

    fn retry_due(&self, now: Instant) -> bool { self.next_try.is_some_and(|at| now >= at) }

    fn failed(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        self.next_try = Some(now + restart_delay(self.failures));
    }

    /// A stream started; it still has to prove itself if capture was down.
    fn started(&mut self, now: Instant) {
        self.next_try = None;
        if self.since.is_some() {
            self.restarted_at = Some(now);
        }
    }

    /// The running stream delivered a frame (`frame`) or nothing.
    fn running(&mut self, now: Instant, frame: bool) {
        if frame || self.restarted_at.is_some_and(|at| now.saturating_duration_since(at) >= RESTART_CONFIRM) {
            *self = Self::default();
        }
    }
}

fn restart_delay(failures: u32) -> Duration {
    FIRST_RESTART_DELAY.saturating_mul(1 << failures.min(8)).min(MAX_RESTART_DELAY)
}

/// Body of the capture thread. Returns when the capturer is dropped.
///
/// A stream that stops (display unplugged or asleep, permission revoked) is
/// restarted with backoff: on the same display, else on the main one. The
/// display list is checked every `monitor_check`; a change is published for
/// the session, and losing the captured display counts as a stop.
fn capture_loop<P: CapturePlatform>(
    mut platform: P,
    stream: P::Stream,
    commands: &std_mpsc::Receiver<CaptureCommand>,
    latest: &LatestFrame,
    monitor_check: Duration,
) {
    let mut stream = Some(stream);
    // The display being captured; `None` is the main display.
    let mut display = None;
    let mut generation = 0;
    let mut known = platform.monitors();
    let mut next_check = Instant::now() + monitor_check;
    let mut recovery = Recovery::default();
    loop {
        match commands.try_recv() {
            Ok(CaptureCommand::SelectMonitor { id, generation: requested }) => {
                // The new engine starts before the old one stops, so a failed
                // switch keeps capturing the current monitor.
                if let Some(next) = platform.start(Some(id)) {
                    stream = Some(next);
                    display = Some(id);
                    generation = requested;
                    recovery.started(Instant::now());
                }
                continue;
            }
            Err(std_mpsc::TryRecvError::Empty) => {}
            Err(std_mpsc::TryRecvError::Disconnected) => return,
        }
        let now = Instant::now();
        if now >= next_check {
            next_check = now + monitor_check;
            let monitors = platform.monitors();
            if monitors != known {
                let captured = display.or_else(|| known.iter().find(|monitor| monitor.is_primary).map(|monitor| monitor.id));
                if stream.is_some() && captured.is_some_and(|id| !monitors.iter().any(|monitor| monitor.id == id)) {
                    log::warn!("the captured display went away");
                    stream = None;
                    recovery.lost(now);
                }
                *latest.monitors.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(monitors.clone());
                known = monitors;
            }
        }
        match &stream {
            Some(current) => match current.newest_frame(CAPTURE_POLL) {
                Ok(Some(frame)) => {
                    latest.publish(generation, frame);
                    recovery.running(Instant::now(), true);
                }
                Ok(None) => recovery.running(Instant::now(), false),
                Err(()) => {
                    log::warn!("screen capture stopped delivering frames; restarting it");
                    stream = None;
                    recovery.lost(Instant::now());
                }
            },
            None if recovery.retry_due(now) => {
                let restarted = display
                    .and_then(|id| platform.start(Some(id)).map(|next| (next, Some(id))))
                    .or_else(|| platform.start(None).map(|next| (next, None)));
                match restarted {
                    Some((next, on)) => {
                        if on != display {
                            log::info!("screen capture moved to the main display");
                        }
                        stream = Some(next);
                        display = on;
                        recovery.started(Instant::now());
                    }
                    None => recovery.failed(Instant::now()),
                }
            }
            None => std::thread::sleep(CAPTURE_POLL),
        }
        let mut stalled = latest.stalled.lock().unwrap_or_else(|poison| poison.into_inner());
        if *stalled != recovery.since {
            *stalled = recovery.since;
        }
    }
}

/// Converts a scap frame to tightly packed BGRA, or `None` if it is empty,
/// malformed or in an unsupported format. PipeWire negotiates RGBx, BGRx,
/// xBGR or RGB regardless of the requested output type.
fn frame_to_bgra(frame: Frame) -> Option<RawFrame> {
    let (layout, width, height, data) = match frame {
        Frame::BGRA(f) => (SourceLayout::Bgra, f.width, f.height, f.data),
        Frame::BGRx(f) => (SourceLayout::Bgrx, f.width, f.height, f.data),
        Frame::RGBx(f) => (SourceLayout::Rgbx, f.width, f.height, f.data),
        Frame::XBGR(f) => (SourceLayout::Xbgr, f.width, f.height, f.data),
        Frame::RGB(f) => (SourceLayout::Rgb, f.width, f.height, f.data),
        _ => return None,
    };
    let width = u16::try_from(width).ok()?;
    let height = u16::try_from(height).ok()?;
    let data = tight_bgra(layout, width, height, data)?;
    Some(RawFrame { width, height, data, format: PixelFormat::Bgra, timestamp_us: 0 })
}

/// Byte order of a captured pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceLayout {
    Bgra,
    Bgrx,
    Rgbx,
    Xbgr,
    Rgb,
}

impl SourceLayout {
    fn bytes_per_pixel(self) -> usize {
        if self == Self::Rgb { 3 } else { 4 }
    }
}

/// Repacks `data`, whose rows may be padded, into tight BGRA rows of
/// `width * 4` bytes. Windows Graphics Capture pads rows at widths such as
/// 1366, 1440 and 3440; PipeWire and ScreenCaptureKit buffers may be padded
/// too. The row stride is inferred as `data.len() / height` and must hold at
/// least one row of pixels.
fn tight_bgra(layout: SourceLayout, width: u16, height: u16, data: Vec<u8>) -> Option<Vec<u8>> {
    let (width, height) = (usize::from(width), usize::from(height));
    if width == 0 || height == 0 {
        return None;
    }
    let row_bytes = width * layout.bytes_per_pixel();
    let stride = data.len() / height;
    if stride < row_bytes {
        return None;
    }
    if layout == SourceLayout::Bgra && data.len() == row_bytes * height {
        return Some(data);
    }
    let mut out = Vec::with_capacity(width * height * 4);
    for row in data.chunks_exact(stride).take(height) {
        let row = &row[..row_bytes];
        match layout {
            SourceLayout::Bgra => out.extend_from_slice(row),
            SourceLayout::Bgrx => {
                for p in row.as_chunks::<4>().0 {
                    out.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
            SourceLayout::Rgbx => {
                for p in row.as_chunks::<4>().0 {
                    out.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
            SourceLayout::Xbgr => {
                for p in row.as_chunks::<4>().0 {
                    out.extend_from_slice(&[p[1], p[2], p[3], 255]);
                }
            }
            SourceLayout::Rgb => {
                for p in row.as_chunks::<3>().0 {
                    out.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
        }
    }
    Some(out)
}

fn selected_monitor_id(monitors: &[MonitorInfo], id: u32) -> Option<u32> {
    monitors.iter().find(|monitor| monitor.id == id).map(|monitor| monitor.id)
}

fn list_monitors() -> Vec<MonitorInfo> {
    scap::get_all_targets()
        .into_iter()
        .enumerate()
        .filter_map(|target| match target {
            (index, scap::Target::Display(display)) => {
                // scap 0.0.8 exposes display IDs and titles publicly, but
                // keeps get_target_dimensions private. Keep the metadata
                // shape stable and report unknown dimensions as zero.
                let (width, height) = (0, 0);
                let primary = index == 0;
                Some(MonitorInfo {
                    id: display.id,
                    name: display.title,
                    width: u32::try_from(width).ok()?,
                    height: u32::try_from(height).ok()?,
                    is_primary: primary,
                    is_virtual: false,
                })
            }
            (_, scap::Target::Window(_)) => None,
        })
        .collect()
}

impl ScreenCapturer for ScapCapturer {
    fn next_frame(&mut self) -> Option<RawFrame> {
        let (generation, frame) =
            self.latest.slot.lock().unwrap_or_else(|poison| poison.into_inner()).take()?;
        (generation == self.latest.generation.load(Ordering::Acquire)).then_some(frame)
    }

    fn wait_frame(&mut self, timeout: Duration) -> Option<RawFrame> {
        let slot = self.latest.slot.lock().unwrap_or_else(|poison| poison.into_inner());
        let (mut slot, _) = self
            .latest
            .arrived
            .wait_timeout_while(slot, timeout, |slot| slot.is_none())
            .unwrap_or_else(|poison| poison.into_inner());
        let (generation, frame) = slot.take()?;
        (generation == self.latest.generation.load(Ordering::Acquire)).then_some(frame)
    }

    fn monitors(&self) -> Vec<MonitorInfo> { list_monitors() }

    fn stalled_since(&self) -> Option<Instant> {
        *self.latest.stalled.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn monitors_changed(&mut self) -> Option<Vec<MonitorInfo>> {
        self.latest.monitors.lock().unwrap_or_else(|poison| poison.into_inner()).take()
    }

    /// Asks the capture thread to switch to monitor `id`. Frames from the
    /// previous monitor are discarded from now on; the session picks up the
    /// new monitor's size from its first frame.
    fn select_monitor(&mut self, id: u32) -> bool {
        if selected_monitor_id(&self.monitors(), id).is_none() {
            return false;
        }
        let Some(commands) = &self.commands else { return false };
        let generation = self.latest.generation.fetch_add(1, Ordering::AcqRel) + 1;
        commands.send(CaptureCommand::SelectMonitor { id, generation }).is_ok()
    }
}

impl Drop for ScapCapturer {
    fn drop(&mut self) {
        // Closing the command channel stops the capture thread within one
        // poll interval (stopping the engine); joining waits for that.
        self.commands.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Test capturer that always has a fresh solid frame of the current size.
/// `wait_frame` paces it at the stream frame rate.
pub struct FakeCapturer {
    w: u16,
    h: u16,
}

impl FakeCapturer {
    pub fn new(w: u16, h: u16) -> Self {
        Self { w, h }
    }

    /// Changes the size of subsequent frames, like a display mode change.
    pub fn set_size(&mut self, w: u16, h: u16) {
        (self.w, self.h) = (w, h);
    }
}

impl ScreenCapturer for FakeCapturer {
    fn wait_frame(&mut self, timeout: Duration) -> Option<RawFrame> {
        std::thread::sleep(timeout.min(Duration::from_secs(1) / STREAM_FPS));
        self.next_frame()
    }

    fn next_frame(&mut self) -> Option<RawFrame> {
        Some(RawFrame {
            width: self.w,
            height: self.h,
            data: vec![0u8; usize::from(self.w) * usize::from(self.h) * 4],
            format: PixelFormat::Bgra,
            timestamp_us: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_capturer_yields_a_frame_of_expected_dims() {
        let mut c = FakeCapturer::new(320, 240);
        let f = c.next_frame().expect("frame");
        assert_eq!((f.width, f.height), (320, 240));
        assert!(!f.data.is_empty());
    }

    #[test]
    fn restarts_back_off_and_a_restart_must_prove_itself() {
        let start = Instant::now();
        let ms = |ms| start + Duration::from_millis(ms);
        let mut recovery = Recovery::default();
        recovery.lost(start);
        assert_eq!(recovery.since, Some(start));
        assert!(!recovery.retry_due(ms(249)));
        assert!(recovery.retry_due(ms(250)));
        recovery.failed(ms(250));
        assert!(!recovery.retry_due(ms(749)));
        assert!(recovery.retry_due(ms(750)));
        assert_eq!(restart_delay(10), MAX_RESTART_DELAY);

        // A restart that fails again keeps the original stall time.
        recovery.started(ms(800));
        recovery.running(ms(900), false);
        recovery.lost(ms(1_000));
        assert_eq!(recovery.since, Some(start));

        // Running quietly long enough, or a frame, ends the stall.
        recovery.started(ms(2_000));
        recovery.running(ms(2_000) + RESTART_CONFIRM, false);
        assert_eq!(recovery.since, None);
        recovery.lost(ms(5_000));
        recovery.started(ms(5_300));
        recovery.running(ms(5_310), true);
        assert_eq!(recovery.since, None);
    }

    /// A stream that delivers a frame per poll until its flag is set.
    struct FakeStream {
        failed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl FrameSource for FakeStream {
        fn newest_frame(&self, _timeout: Duration) -> Result<Option<RawFrame>, ()> {
            if self.failed.load(Ordering::SeqCst) {
                return Err(());
            }
            std::thread::sleep(Duration::from_millis(5));
            Ok(FakeCapturer::new(4, 4).next_frame())
        }
    }

    /// Displays and start results the test controls; records every start.
    #[derive(Clone, Default)]
    struct FakePlatform {
        monitors: Arc<Mutex<Vec<MonitorInfo>>>,
        starts: Arc<Mutex<Vec<Option<u32>>>>,
        /// Starts to refuse before one succeeds.
        refuse: Arc<std::sync::atomic::AtomicUsize>,
        /// Fails the stream started last.
        current: Arc<Mutex<Arc<std::sync::atomic::AtomicBool>>>,
    }

    impl FakePlatform {
        fn with_monitors(ids: &[u32]) -> Self {
            let platform = Self::default();
            *platform.monitors.lock().unwrap() = monitors(ids);
            platform
        }

        fn fail_stream(&self) { self.current.lock().unwrap().store(true, Ordering::SeqCst); }

        fn starts(&self) -> Vec<Option<u32>> { self.starts.lock().unwrap().clone() }
    }

    impl CapturePlatform for FakePlatform {
        type Stream = FakeStream;

        fn start(&mut self, display: Option<u32>) -> Option<FakeStream> {
            self.starts.lock().unwrap().push(display);
            let listed = display.is_none_or(|id| self.monitors.lock().unwrap().iter().any(|monitor| monitor.id == id));
            let left = self.refuse.load(Ordering::SeqCst);
            let refused = left > 0;
            if refused { self.refuse.store(left - 1, Ordering::SeqCst); }
            if !listed || refused {
                return None;
            }
            let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            *self.current.lock().unwrap() = Arc::clone(&failed);
            Some(FakeStream { failed })
        }

        fn monitors(&mut self) -> Vec<MonitorInfo> { self.monitors.lock().unwrap().clone() }
    }

    fn monitors(ids: &[u32]) -> Vec<MonitorInfo> {
        ids.iter().enumerate().map(|(index, &id)| MonitorInfo { id, name: format!("D{id}"), width: 0, height: 0, is_primary: index == 0, is_virtual: false }).collect()
    }

    struct Loop {
        latest: Arc<LatestFrame>,
        commands: Option<std_mpsc::Sender<CaptureCommand>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Loop {
        fn start(mut platform: FakePlatform, monitor_check: Duration) -> Self {
            let latest = Arc::new(LatestFrame::default());
            let (commands, command_rx) = std_mpsc::channel();
            let shared = Arc::clone(&latest);
            let stream = platform.start(None).expect("first stream");
            let thread = std::thread::spawn(move || capture_loop(platform, stream, &command_rx, &shared, monitor_check));
            Self { latest, commands: Some(commands), thread: Some(thread) }
        }

        fn stalled(&self) -> bool { self.latest.stalled.lock().unwrap().is_some() }

        /// Waits for `condition`, up to two seconds.
        fn until(&self, what: &str, condition: impl Fn(&Self) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !condition(self) {
                assert!(Instant::now() < deadline, "timed out waiting until {what}");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn fresh_frame(&self) -> bool {
            self.latest.slot.lock().unwrap().take();
            self.until("a frame arrives", |capture| capture.latest.slot.lock().unwrap().is_some());
            true
        }
    }

    impl Drop for Loop {
        fn drop(&mut self) {
            self.commands.take();
            if let Some(thread) = self.thread.take() { thread.join().unwrap(); }
        }
    }

    #[test]
    fn a_stopped_capture_restarts_with_backoff() {
        let platform = FakePlatform::with_monitors(&[1]);
        let capture = Loop::start(platform.clone(), Duration::from_secs(3600));
        assert!(capture.fresh_frame());

        platform.refuse.store(2, Ordering::SeqCst);
        platform.fail_stream();
        capture.until("the stall is reported", Loop::stalled);
        capture.until("capture runs again", |capture| !capture.stalled());
        assert!(capture.fresh_frame());
        // The first stream, two refused restarts, then the one that worked.
        assert_eq!(platform.starts(), [None, None, None, None]);
    }

    #[test]
    fn a_lost_display_falls_back_to_the_main_one() {
        let platform = FakePlatform::with_monitors(&[1, 7]);
        let capture = Loop::start(platform.clone(), Duration::from_millis(20));
        let generation = capture.latest.generation.fetch_add(1, Ordering::AcqRel) + 1;
        capture.commands.as_ref().unwrap().send(CaptureCommand::SelectMonitor { id: 7, generation }).unwrap();
        capture.until("monitor 7 is captured", |_| platform.starts().len() == 2);

        // Unplugged: the list changes and the stream on it is given up.
        *platform.monitors.lock().unwrap() = monitors(&[1]);
        capture.until("the new list is published", |capture| capture.latest.monitors.lock().unwrap().is_some());
        assert_eq!(capture.latest.monitors.lock().unwrap().take(), Some(monitors(&[1])));
        capture.until("capture runs again", |capture| platform.starts().len() == 4 && !capture.stalled());
        assert!(capture.fresh_frame());
        assert_eq!(platform.starts(), [None, Some(7), Some(7), None], "the same display first, then the main one");
    }

    #[test]
    fn selected_monitor_id_only_accepts_advertised_displays() {
        let monitors = vec![MonitorInfo { id: 4, name: "A".into(), width: 0, height: 0, is_primary: true, is_virtual: false }];
        assert_eq!(selected_monitor_id(&monitors, 4), Some(4));
        assert_eq!(selected_monitor_id(&monitors, 9), None);
    }

    /// `height` rows of `width` BGRA pixels numbered from 1, each row padded
    /// with `padding` bytes of 0xEE.
    fn padded_bgra(width: usize, height: usize, padding: usize) -> Vec<u8> {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let n = (y * width + x + 1) as u8;
                data.extend_from_slice(&[n, n, n, n]);
            }
            data.extend(std::iter::repeat_n(0xEE, padding));
        }
        data
    }

    #[test]
    fn tight_bgra_keeps_unpadded_frames() {
        let data = padded_bgra(3, 2, 0);
        assert_eq!(tight_bgra(SourceLayout::Bgra, 3, 2, data.clone()), Some(data));
    }

    #[test]
    fn tight_bgra_strips_row_padding() {
        // 1366 BGRA pixels are 5464 bytes, which WGC pads to a 5504-byte
        // (64-byte aligned) stride.
        let (width, height) = (1366, 4);
        let padded = padded_bgra(width, height, 5504 - width * 4);
        assert_eq!(padded.len(), 5504 * height);
        let tight = tight_bgra(SourceLayout::Bgra, 1366, 4, padded).expect("compacted");
        assert_eq!(tight, padded_bgra(width, height, 0));

        // A small frame with a 2-pixel row pad.
        let tight = tight_bgra(SourceLayout::Bgra, 3, 2, padded_bgra(3, 2, 8)).expect("compacted");
        assert_eq!(tight, padded_bgra(3, 2, 0));
    }

    #[test]
    fn tight_bgra_rejects_short_or_empty_frames() {
        assert_eq!(tight_bgra(SourceLayout::Bgra, 3, 2, vec![0; 3 * 2 * 4 - 1]), None);
        assert_eq!(tight_bgra(SourceLayout::Bgra, 0, 2, vec![0; 8]), None);
        assert_eq!(tight_bgra(SourceLayout::Bgra, 3, 0, vec![0; 8]), None);
    }

    #[test]
    fn tight_bgra_converts_pipewire_layouts() {
        let bgra = [10, 20, 30, 255];
        assert_eq!(tight_bgra(SourceLayout::Bgrx, 1, 1, vec![10, 20, 30, 0]).unwrap(), bgra);
        assert_eq!(tight_bgra(SourceLayout::Rgbx, 1, 1, vec![30, 20, 10, 0]).unwrap(), bgra);
        assert_eq!(tight_bgra(SourceLayout::Xbgr, 1, 1, vec![0, 10, 20, 30]).unwrap(), bgra);
        // RGB rows padded from 6 to 8 bytes.
        assert_eq!(
            tight_bgra(SourceLayout::Rgb, 2, 1, vec![30, 20, 10, 3, 2, 1, 0, 0]).unwrap(),
            [10, 20, 30, 255, 1, 2, 3, 255]
        );
    }
}
