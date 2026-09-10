use crate::encode::{PixelFormat, RawFrame};
use scap::capturer::{Capturer, Options};
use scap::frame::{Frame, FrameType};

pub trait ScreenCapturer {
    fn next_frame(&mut self) -> Option<RawFrame>;
}

pub struct ScapCapturer {
    capturer: Capturer,
}

impl ScapCapturer {
    pub fn new() -> Option<Self> {
        if !scap::is_supported() {
            return None;
        }

        if !scap::has_permission() && !scap::request_permission() {
            return None;
        }

        let options = Options {
            fps: 15,
            target: None,
            output_type: FrameType::BGRAFrame,
            ..Default::default()
        };
        let mut capturer = Capturer::build(options).ok()?;
        capturer.start_capture();

        Some(Self { capturer })
    }
}

impl ScreenCapturer for ScapCapturer {
    fn next_frame(&mut self) -> Option<RawFrame> {
        let frame = self.capturer.get_next_frame().ok()?;
        let Frame::BGRA(frame) = frame else {
            return None;
        };

        let width = u16::try_from(frame.width).ok()?;
        let height = u16::try_from(frame.height).ok()?;
        let timestamp_us = (frame.display_time / 1_000).try_into().unwrap_or(u32::MAX);

        Some(RawFrame {
            width,
            height,
            data: frame.data,
            format: PixelFormat::Bgra,
            timestamp_us,
        })
    }
}

impl Drop for ScapCapturer {
    fn drop(&mut self) {
        self.capturer.stop_capture();
    }
}

pub struct FakeCapturer {
    w: u16,
    h: u16,
}

impl FakeCapturer {
    pub fn new(w: u16, h: u16) -> Self {
        Self { w, h }
    }
}

impl ScreenCapturer for FakeCapturer {
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
