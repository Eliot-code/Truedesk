//! A [`ScreenCapturer`] fed from outside the crate: the Android app captures
//! the screen with MediaProjection and pushes each frame in through
//! [`FramePusher::push`], and each host session pulls frames through its own
//! [`PushCapturer`]. The slot keeps only the newest frame, so a slow encoder
//! skips stale frames instead of queueing them.
//!
//! Android only produces a frame when the screen changes, so the slot keeps
//! the newest frame even after a session has read it: a session that starts
//! while the screen is still gets the current picture straight away.

use host_core::capture::ScreenCapturer;
use host_core::encode::{PixelFormat, RawFrame};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Default)]
struct Latest {
    /// Bumped by every push; 0 means nothing was pushed yet.
    seq: u64,
    frame: Option<RawFrame>,
}

#[derive(Default)]
struct Slot {
    latest: Mutex<Latest>,
    arrived: Condvar,
}

impl Slot {
    fn lock(&self) -> MutexGuard<'_, Latest> {
        self.latest.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The app's end: hands captured frames to host sessions. Cheap to clone.
#[derive(Clone, Default)]
pub struct FramePusher {
    slot: Arc<Slot>,
}

impl FramePusher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes one frame. `rgba` holds `height` rows of `stride` bytes
    /// (at least `width * 4`; 0 means exactly that), R,G,B,A per pixel, as
    /// Android's `PixelFormat.RGBA_8888` lays them out. A buffer too small
    /// for that is dropped.
    pub fn push(&self, width: u16, height: u16, rgba: &[u8], stride: usize) {
        let Some(frame) = pack_rgba(width, height, rgba, stride) else { return };
        let mut latest = self.slot.lock();
        latest.seq += 1;
        latest.frame = Some(frame);
        drop(latest);
        self.slot.arrived.notify_all();
    }

    /// Forgets the newest frame, so a session that starts after capture
    /// stopped shows no stale picture.
    pub fn clear(&self) {
        self.slot.lock().frame = None;
    }

    /// A capturer for one host session. It starts with the newest frame.
    pub fn capturer(&self) -> PushCapturer {
        PushCapturer { slot: Arc::clone(&self.slot), seen: 0 }
    }
}

/// One session's end: returns each newer frame once.
pub struct PushCapturer {
    slot: Arc<Slot>,
    /// The `seq` of the last frame this capturer returned.
    seen: u64,
}

impl PushCapturer {
    fn take_newer(&mut self, latest: &Latest) -> Option<RawFrame> {
        if latest.seq <= self.seen {
            return None;
        }
        self.seen = latest.seq;
        latest.frame.clone()
    }
}

impl ScreenCapturer for PushCapturer {
    fn next_frame(&mut self) -> Option<RawFrame> {
        let slot = Arc::clone(&self.slot);
        let latest = slot.lock();
        self.take_newer(&latest)
    }

    fn wait_frame(&mut self, timeout: Duration) -> Option<RawFrame> {
        let slot = Arc::clone(&self.slot);
        let seen = self.seen;
        let (latest, _) = slot
            .arrived
            .wait_timeout_while(slot.lock(), timeout, |latest| latest.seq <= seen)
            .unwrap_or_else(|p| p.into_inner());
        self.take_newer(&latest)
    }
}

/// Repacks captured RGBA rows into a tight BGRA [`RawFrame`] (the pixel
/// order the encoder takes). `None` if the buffer is too small for the
/// stated size and stride.
fn pack_rgba(width: u16, height: u16, src: &[u8], stride: usize) -> Option<RawFrame> {
    let (w, h) = (usize::from(width), usize::from(height));
    if w == 0 || h == 0 {
        return None;
    }
    let row = w * 4;
    let stride = stride.max(row);
    // The last row needs only `row` bytes: Android may not pad it.
    if src.len() < stride * (h - 1) + row {
        return None;
    }
    let mut data = vec![0u8; row * h];
    for (y, dst_row) in data.chunks_exact_mut(row).enumerate() {
        let src_row = &src[y * stride..y * stride + row];
        for (dst, px) in dst_row.as_chunks_mut::<4>().0.iter_mut().zip(src_row.as_chunks::<4>().0) {
            // RGBA -> BGRA. The screen is opaque; its alpha byte is ignored.
            *dst = [px[2], px[1], px[0], 255];
        }
    }
    Some(RawFrame { width, height, data, format: PixelFormat::Bgra, timestamp_us: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_then_pull_repacks_rgba_to_bgra() {
        let pusher = FramePusher::new();
        let mut capturer = pusher.capturer();
        // A red and a half-transparent green pixel, 2x1.
        pusher.push(2, 1, &[255, 0, 0, 255, 0, 255, 0, 128], 0);
        let frame = capturer.next_frame().expect("a frame");
        assert_eq!((frame.width, frame.height), (2, 1));
        assert_eq!(frame.data, vec![0, 0, 255, 255, 0, 255, 0, 255]);
        // Each frame is returned once.
        assert!(capturer.next_frame().is_none());
    }

    #[test]
    fn stride_padding_is_stripped() {
        let pusher = FramePusher::new();
        let mut capturer = pusher.capturer();
        // 1x2, rows padded from 4 to 8 bytes; the last row is not padded.
        pusher.push(1, 2, &[1, 2, 3, 255, 0, 0, 0, 0, 10, 20, 30, 255], 8);
        let frame = capturer.next_frame().expect("a frame");
        assert_eq!(frame.data, vec![3, 2, 1, 255, 30, 20, 10, 255]);
    }

    #[test]
    fn only_the_newest_frame_is_kept() {
        let pusher = FramePusher::new();
        let mut capturer = pusher.capturer();
        pusher.push(1, 1, &[1, 1, 1, 255], 0);
        pusher.push(1, 1, &[2, 2, 2, 255], 0);
        assert_eq!(capturer.next_frame().expect("a frame").data, vec![2, 2, 2, 255]);
        assert!(capturer.next_frame().is_none());
    }

    #[test]
    fn a_new_session_starts_with_the_current_screen() {
        let pusher = FramePusher::new();
        let mut first = pusher.capturer();
        pusher.push(1, 1, &[7, 7, 7, 255], 0);
        assert!(first.next_frame().is_some());
        // The screen stays still; a later session still gets the picture.
        let mut second = pusher.capturer();
        assert_eq!(second.next_frame().expect("a frame").data, vec![7, 7, 7, 255]);
    }

    #[test]
    fn cleared_frames_are_not_shown_to_new_sessions() {
        let pusher = FramePusher::new();
        pusher.push(1, 1, &[7, 7, 7, 255], 0);
        pusher.clear();
        assert!(pusher.capturer().next_frame().is_none());
    }

    #[test]
    fn undersized_buffers_are_dropped() {
        let pusher = FramePusher::new();
        pusher.push(4, 4, &[0; 8], 0);
        pusher.push(0, 4, &[0; 64], 0);
        assert!(pusher.capturer().next_frame().is_none());
    }

    #[test]
    fn wait_frame_times_out_without_a_new_frame() {
        let pusher = FramePusher::new();
        let mut capturer = pusher.capturer();
        assert!(capturer.wait_frame(Duration::from_millis(10)).is_none());
        pusher.push(1, 1, &[9, 9, 9, 255], 0);
        assert!(capturer.wait_frame(Duration::from_millis(10)).is_some());
        assert!(capturer.wait_frame(Duration::from_millis(10)).is_none());
    }

    #[test]
    fn wait_frame_wakes_up_for_a_push() {
        let pusher = FramePusher::new();
        let mut capturer = pusher.capturer();
        let feeder = pusher.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            feeder.push(1, 1, &[5, 5, 5, 255], 0);
        });
        assert!(capturer.wait_frame(Duration::from_secs(5)).is_some());
        handle.join().unwrap();
    }
}
