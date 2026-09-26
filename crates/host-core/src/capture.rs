use crate::encode::{PixelFormat, RawFrame};
use scap::capturer::{Capturer, Options};
use scap::frame::{Frame, FrameType};
use protocol::control::MonitorInfo;

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

pub trait ScreenCapturer {
    fn next_frame(&mut self) -> Option<RawFrame>;

    fn monitors(&self) -> Vec<MonitorInfo> { Vec::new() }

    fn select_monitor(&mut self, _id: u32) -> bool { false }
}

pub struct ScapCapturer {
    capturer: Capturer,
}

impl ScapCapturer {
    pub fn new() -> Option<Self> {
        if !scap::is_supported() {
            return None;
        }

        if !capture_permission_available() {
            return None;
        }

        let options = Self::options(None);
        let mut capturer = Capturer::build(options).ok()?;
        capturer.start_capture();

        Some(Self { capturer })
    }

    fn options(target: Option<scap::Target>) -> Options {
        Options {
            fps: 15,
            target,
            output_type: FrameType::BGRAFrame,
            ..Default::default()
        }
    }
}

fn selected_monitor_id(monitors: &[MonitorInfo], id: u32) -> Option<u32> {
    monitors.iter().find(|monitor| monitor.id == id).map(|monitor| monitor.id)
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

    fn select_monitor(&mut self, id: u32) -> bool {
        let monitors = self.monitors();
        if selected_monitor_id(&monitors, id).is_none() { return false; }
        let target = scap::get_all_targets().into_iter().find_map(|target| match target {
            scap::Target::Display(display) if display.id == id => Some(scap::Target::Display(display)),
            _ => None,
        });
        let Some(target) = target else { return false };
        self.capturer.stop_capture();
        let Ok(mut capturer) = Capturer::build(Self::options(Some(target))) else { return false };
        capturer.start_capture();
        self.capturer = capturer;
        true
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

    #[test]
    fn selected_monitor_id_only_accepts_advertised_displays() {
        let monitors = vec![MonitorInfo { id: 4, name: "A".into(), width: 0, height: 0, is_primary: true }];
        assert_eq!(selected_monitor_id(&monitors, 4), Some(4));
        assert_eq!(selected_monitor_id(&monitors, 9), None);
    }
}
