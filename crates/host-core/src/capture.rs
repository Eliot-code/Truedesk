use crate::encode::{stream::STREAM_FPS, PixelFormat, RawFrame};
use protocol::control::MonitorInfo;
use std::time::{Duration, Instant};

// Desktop capture goes through scap. Phones capture the screen outside
// this crate (the Android app) and pass frames in through their own
// ScreenCapturer.
#[cfg(not(target_os = "android"))]
mod desktop;
#[cfg(target_os = "windows")]
mod secure_desktop;

#[cfg(not(target_os = "android"))]
pub use desktop::ScapCapturer;

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

    /// Whether a second, independent instance of this capturer can run at the
    /// same time, so the host can stream extra monitors as separate aux
    /// streams. False by default; the secure-desktop capturer keeps it off.
    fn supports_aux_streams(&self) -> bool { false }

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
    fn supports_aux_streams(&self) -> bool { (**self).supports_aux_streams() }
    fn stalled_since(&self) -> Option<Instant> { (**self).stalled_since() }
    fn monitors_changed(&mut self) -> Option<Vec<MonitorInfo>> { (**self).monitors_changed() }
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
}
