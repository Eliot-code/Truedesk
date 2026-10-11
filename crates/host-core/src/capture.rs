use crate::encode::{stream::STREAM_FPS, PixelFormat, RawFrame};
use protocol::control::MonitorInfo;
use scap::capturer::engine::{ChannelItem, Engine};
use scap::capturer::Options;
use scap::frame::{Frame, FrameType};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

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
}

/// How long the capture thread waits for a frame before it checks for
/// monitor switches and shutdown.
const CAPTURE_POLL: Duration = Duration::from_millis(20);

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
    pub fn new() -> Option<Self> {
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
                let Some(stream) = CaptureStream::start(None) else {
                    let _ = ready_tx.send(false);
                    return;
                };
                let _ = ready_tx.send(true);
                capture_loop(stream, &command_rx, &shared);
            })
            .ok()?;
        if !ready_rx.recv().unwrap_or(false) {
            let _ = worker.join();
            return None;
        }
        Some(Self { latest, commands: Some(commands), worker: Some(worker) })
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

/// Body of the capture thread. Returns when the capturer is dropped.
fn capture_loop(
    mut stream: CaptureStream,
    commands: &std_mpsc::Receiver<CaptureCommand>,
    latest: &LatestFrame,
) {
    let mut generation = 0;
    loop {
        match commands.try_recv() {
            Ok(CaptureCommand::SelectMonitor { id, generation: requested }) => {
                // The new engine starts before the old one stops, so a failed
                // switch keeps capturing the current monitor.
                if let Some(next) = CaptureStream::start_display(id) {
                    stream = next;
                    generation = requested;
                }
                continue;
            }
            Err(std_mpsc::TryRecvError::Empty) => {}
            Err(std_mpsc::TryRecvError::Disconnected) => return,
        }
        match stream.newest_frame(CAPTURE_POLL) {
            Ok(Some(frame)) => {
                *latest.slot.lock().unwrap_or_else(|poison| poison.into_inner()) =
                    Some((generation, frame));
                latest.arrived.notify_all();
            }
            Ok(None) => {}
            Err(()) => {
                log::warn!("screen capture stopped delivering frames");
                // Only a monitor switch can restart capture now.
                loop {
                    let Ok(CaptureCommand::SelectMonitor { id, generation: requested }) =
                        commands.recv()
                    else {
                        return;
                    };
                    if let Some(next) = CaptureStream::start_display(id) {
                        stream = next;
                        generation = requested;
                        break;
                    }
                }
            }
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

    fn monitors(&self) -> Vec<MonitorInfo> {
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
                    })
                }
                (_, scap::Target::Window(_)) => None,
            })
            .collect()
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
    fn selected_monitor_id_only_accepts_advertised_displays() {
        let monitors = vec![MonitorInfo { id: 4, name: "A".into(), width: 0, height: 0, is_primary: true }];
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
