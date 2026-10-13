//! Windows desktop capture used by the SYSTEM-launched host.
//!
//! Windows Graphics Capture is tied to the interactive user's desktop. A
//! service-launched process must instead capture the desktop currently owned
//! by the input desktop. GDI's desktop DC follows that desktop, including
//! Winlogon and consent/UAC desktops, when the process is running in the
//! active console session.

use super::ScreenCapturer;
use crate::encode::{PixelFormat, RawFrame};
use protocol::control::MonitorInfo;
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
    GetDIBits, GetDeviceCaps, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HGDIOBJ, HORZRES, SRCCOPY, VERTRES,
};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, OpenInputDesktop, SetThreadDesktop, DESKTOP_READOBJECTS,
};

pub struct SecureDesktopCapturer {
    width: u16,
    height: u16,
    monitor: MonitorInfo,
    desktop_selected: bool,
}

impl SecureDesktopCapturer {
    pub fn new() -> Option<Self> {
        let dc = unsafe { GetDC(HWND(0)) };
        if dc.is_invalid() {
            return None;
        }
        let width = unsafe { GetDeviceCaps(dc, HORZRES) };
        let height = unsafe { GetDeviceCaps(dc, VERTRES) };
        unsafe { ReleaseDC(HWND(0), dc) };
        let width = u16::try_from(width).ok()?.min(u16::MAX);
        let height = u16::try_from(height).ok()?.min(u16::MAX);
        (width > 0 && height > 0).then(|| Self {
            width,
            height,
            monitor: MonitorInfo {
                id: 0,
                name: "Windows desktop".into(),
                width: u32::from(width),
                height: u32::from(height),
                is_primary: true,
                is_virtual: false,
            },
            desktop_selected: false,
        })
    }

    fn capture(&self) -> Option<RawFrame> {
        let width = i32::from(self.width);
        let height = i32::from(self.height);
        unsafe {
            let source = GetDC(HWND(0));
            if source.is_invalid() {
                return None;
            }
            let memory = CreateCompatibleDC(source);
            if memory.is_invalid() {
                ReleaseDC(HWND(0), source);
                return None;
            }
            let bitmap = CreateCompatibleBitmap(source, width, height);
            if bitmap.is_invalid() {
                DeleteDC(memory);
                ReleaseDC(HWND(0), source);
                return None;
            }
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            let copied = BitBlt(memory, 0, 0, width, height, source, 0, 0, SRCCOPY).is_ok();
            let mut data = vec![0u8; usize::from(self.width) * usize::from(self.height) * 4];
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let rows = if copied {
                GetDIBits(memory, bitmap, 0, height as u32, Some(data.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS)
            } else {
                0
            };
            SelectObject(memory, previous);
            DeleteObject(HGDIOBJ(bitmap.0));
            DeleteDC(memory);
            ReleaseDC(HWND(0), source);
            (copied && rows == height).then_some(RawFrame {
                width: self.width,
                height: self.height,
                data,
                format: PixelFormat::Bgra,
                timestamp_us: 0,
            })
        }
    }

    fn select_input_desktop(&mut self) -> bool {
        if self.desktop_selected {
            return true;
        }
        unsafe {
            let desktop = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS).ok();
            let Some(desktop) = desktop else { return false };
            let selected = SetThreadDesktop(desktop).is_ok();
            let _ = CloseDesktop(desktop);
            self.desktop_selected = selected;
            selected
        }
    }
}

impl ScreenCapturer for SecureDesktopCapturer {
    fn next_frame(&mut self) -> Option<RawFrame> {
        self.select_input_desktop().then(|| self.capture()).flatten()
    }

    fn wait_frame(&mut self, timeout: Duration) -> Option<RawFrame> {
        std::thread::sleep(timeout.min(Duration::from_millis(33)));
        self.next_frame()
    }

    fn monitors(&self) -> Vec<MonitorInfo> { vec![self.monitor.clone()] }

    fn select_monitor(&mut self, id: u32) -> bool { id == self.monitor.id }
}
