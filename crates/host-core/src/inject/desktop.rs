use super::{map_to_bounds, norm_to_pixel, DisplayBounds, InputInjector};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use protocol::input::InputEvent;

pub struct EnigoInjector {
    enigo: Enigo,
    display: Option<u32>,
}

impl EnigoInjector {
    pub fn new() -> Option<Self> {
        // enigo >= 0.3 checks macOS Accessibility trust here and fails when it
        // is missing. Don't let it pop the system prompt: this constructor
        // doubles as the UI's permission probe, which links to the settings
        // pane itself.
        let settings = Settings { open_prompt_to_get_permissions: false, ..Settings::default() };
        Enigo::new(&settings)
            .ok()
            .map(|enigo| Self { enigo, display: None })
    }
}

impl InputInjector for EnigoInjector {
    fn inject(&mut self, ev: &InputEvent, screen_w: u32, screen_h: u32) {
        match *ev {
            InputEvent::MouseMove { x, y } => match platform::display_bounds(self.display) {
                Some(bounds) => {
                    let (x, y) = map_to_bounds(x, y, bounds);
                    platform::move_pointer(&mut self.enigo, x, y);
                }
                // No display geometry (Linux): fall back to the capture size,
                // which matches the pointer space for a single X11 screen.
                None => {
                    let _ = self.enigo.move_mouse(
                        norm_to_pixel(x, screen_w),
                        norm_to_pixel(y, screen_h),
                        Coordinate::Abs,
                    );
                }
            },
            InputEvent::MouseButton { button, down } => {
                let Some(button) = map_button(button) else {
                    return;
                };
                let _ = self.enigo.button(
                    button,
                    if down {
                        Direction::Press
                    } else {
                        Direction::Release
                    },
                );
            }
            InputEvent::Scroll { dx, dy } => {
                // Enigo exposes scrolling as signed clicks on one axis. Send both
                // components independently; zero values are harmless no-ops.
                if dx != 0 {
                    let _ = self.enigo.scroll(dx, Axis::Horizontal);
                }
                if dy != 0 {
                    let _ = self.enigo.scroll(dy, Axis::Vertical);
                }
            }
            InputEvent::Key {
                code,
                down,
                modifiers,
            } => {
                let Some(key) = map_key(code) else { return };
                inject_key(&mut self.enigo, key, down, modifiers, code);
            }
            InputEvent::Char {
                ch,
                down,
                modifiers,
            } => {
                let Some(key) = map_char(ch) else { return };
                inject_key(&mut self.enigo, key, down, modifiers, 0);
            }
        }
    }

    fn select_display(&mut self, display: Option<u32>) {
        self.display = display;
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::DisplayBounds;
    use enigo::{Coordinate, Enigo, Mouse};

    #[repr(C)]
    struct CGRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    // CoreGraphics is already linked through enigo and scap.
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
    }

    /// The display's bounds in global points, the space CGEvent (and so
    /// enigo) positions the pointer in. A display that is gone reports an
    /// empty rect; the main display is used then.
    pub fn display_bounds(display: Option<u32>) -> Option<DisplayBounds> {
        let bounds = |id: u32| {
            // SAFETY: plain CoreGraphics queries; unknown ids yield an empty
            // rect rather than an error.
            let rect = unsafe { CGDisplayBounds(id) };
            (rect.width >= 1.0 && rect.height >= 1.0).then_some(DisplayBounds {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            })
        };
        display.and_then(bounds).or_else(|| bounds(unsafe { CGMainDisplayID() }))
    }

    pub fn move_pointer(enigo: &mut Enigo, x: i32, y: i32) {
        let _ = enigo.move_mouse(x, y, Coordinate::Abs);
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::DisplayBounds;
    use enigo::{Coordinate, Enigo, Mouse};
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, HMONITOR, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    /// The monitor's rectangle on the virtual screen. Monitor ids are the
    /// capturer's HMONITOR values; a stale one falls back to the primary.
    pub fn display_bounds(display: Option<u32>) -> Option<DisplayBounds> {
        let bounds = |monitor: HMONITOR| {
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            // SAFETY: `info` is a valid MONITORINFO with cbSize set; an
            // invalid handle makes the call fail.
            if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
                return None;
            }
            let rect = info.rcMonitor;
            (rect.right > rect.left && rect.bottom > rect.top).then_some(DisplayBounds {
                x: f64::from(rect.left),
                y: f64::from(rect.top),
                width: f64::from(rect.right - rect.left),
                height: f64::from(rect.bottom - rect.top),
            })
        };
        display
            .and_then(|id| bounds(HMONITOR(id as usize as *mut core::ffi::c_void)))
            // SAFETY: MonitorFromPoint has no preconditions.
            .or_else(|| bounds(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }))
    }

    fn virtual_screen() -> DisplayBounds {
        // SAFETY: GetSystemMetrics has no preconditions.
        let metric = |index| f64::from(unsafe { GetSystemMetrics(index) });
        DisplayBounds {
            x: metric(SM_XVIRTUALSCREEN),
            y: metric(SM_YVIRTUALSCREEN),
            width: metric(SM_CXVIRTUALSCREEN),
            height: metric(SM_CYVIRTUALSCREEN),
        }
    }

    /// Moves the pointer to virtual-screen pixel (x, y). Enigo's absolute
    /// moves only cover the primary monitor, so this sends the move itself
    /// relative to the whole virtual desktop.
    pub fn move_pointer(enigo: &mut Enigo, x: i32, y: i32) {
        let Some((dx, dy)) = super::virtual_desk_absolute(x, y, virtual_screen()) else {
            let _ = enigo.move_mouse(x, y, Coordinate::Abs);
            return;
        };
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: one fully initialized INPUT with its correct size.
        unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::DisplayBounds;
    use enigo::Enigo;

    /// Not available: X11 maps the whole root window and PipeWire capture
    /// gives no monitor geometry, so the capture size is used instead.
    pub fn display_bounds(_display: Option<u32>) -> Option<DisplayBounds> {
        None
    }

    pub fn move_pointer(_enigo: &mut Enigo, _x: i32, _y: i32) {}
}

/// Converts virtual-screen pixel (x, y) to the 0..=65535 range that
/// `SendInput` uses with `MOUSEEVENTF_VIRTUALDESK`. `None` if the virtual
/// screen is empty. Windows turns the value back into a pixel by scaling
/// with `size / 65536` and truncating, so this rounds up to land on the
/// pixel itself rather than its left or upper neighbour.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn virtual_desk_absolute(x: i32, y: i32, screen: DisplayBounds) -> Option<(i32, i32)> {
    if screen.width < 1.0 || screen.height < 1.0 {
        return None;
    }
    let axis = |pixel: i32, origin: f64, size: f64| {
        let offset = (f64::from(pixel) - origin).clamp(0.0, size - 1.0);
        ((offset * 65_536.0 / size).ceil() as i32).min(65_535)
    };
    Some((axis(x, screen.x, screen.width), axis(y, screen.y, screen.height)))
}

fn map_button(button: u8) -> Option<Button> {
    match button {
        1 => Some(Button::Left),
        2 => Some(Button::Right),
        3 => Some(Button::Middle),
        _ => None,
    }
}

// Printable key codes are Unicode scalar values from KeyboardEvent.key. Named
// keys retain their browser/DOM-style virtual key codes for compatibility.
fn map_key(code: u32) -> Option<Key> {
    match code {
        8 => Some(Key::Backspace),
        9 => Some(Key::Tab),
        13 => Some(Key::Return),
        16 => Some(Key::Shift),
        17 => Some(Key::Control),
        18 => Some(Key::Alt),
        20 => Some(Key::CapsLock),
        27 => Some(Key::Escape),
        91 | 93 => Some(Key::Meta),
        32 => Some(Key::Space),
        33 => Some(Key::PageUp),
        34 => Some(Key::PageDown),
        35 => Some(Key::End),
        36 => Some(Key::Home),
        37 => Some(Key::LeftArrow),
        38 => Some(Key::UpArrow),
        39 => Some(Key::RightArrow),
        40 => Some(Key::DownArrow),
        46 => Some(Key::Delete),
        112..=123 => Some(match code {
            112 => Key::F1,
            113 => Key::F2,
            114 => Key::F3,
            115 => Key::F4,
            116 => Key::F5,
            117 => Key::F6,
            118 => Key::F7,
            119 => Key::F8,
            120 => Key::F9,
            121 => Key::F10,
            122 => Key::F11,
            _ => Key::F12,
        }),
        65..=90 | 48..=57 => char::from_u32(code).map(Key::Unicode),
        _ => None,
    }
}

fn map_char(ch: char) -> Option<Key> {
    (!ch.is_control()).then_some(Key::Unicode(ch))
}

fn inject_key(enigo: &mut Enigo, key: Key, down: bool, modifiers: u8, code: u32) {
    if down {
        for modifier in modifier_keys(modifiers, code) {
            let _ = enigo.key(modifier, Direction::Press);
        }
        let _ = enigo.key(key, Direction::Press);
    } else {
        let _ = enigo.key(key, Direction::Release);
        for modifier in modifier_keys(modifiers, code).rev() {
            let _ = enigo.key(modifier, Direction::Release);
        }
    }
}

fn modifier_keys(modifiers: u8, code: u32) -> impl DoubleEndedIterator<Item = Key> {
    [
        (1, 16, Key::Shift),
        (1 << 1, 17, Key::Control),
        (1 << 2, 18, Key::Alt),
        (1 << 3, 91, Key::Meta),
    ]
    .into_iter()
    .filter(move |(_, key_code, _)| *key_code != code)
    .filter(move |(mask, _, _)| modifiers & mask != 0)
    .map(|(_, _, key)| key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f64, y: f64, width: f64, height: f64) -> DisplayBounds {
        DisplayBounds { x, y, width, height }
    }

    #[test]
    fn virtual_desk_absolute_round_trips_every_pixel() {
        // Two 1920x1080 monitors, the secondary left of the primary: the
        // virtual screen spans x -1920..1920.
        let screen = bounds(-1920.0, 0.0, 3840.0, 1080.0);
        for x in [-1920, -1919, -1, 0, 1, 959, 1919] {
            let (dx, _) = virtual_desk_absolute(x, 0, screen).unwrap();
            assert!((0..=65_535).contains(&dx));
            // Windows: pixel = origin + trunc(dx * size / 65536).
            let back = -1920 + (i64::from(dx) * 3840 / 65_536) as i32;
            assert_eq!(back, x, "pixel {x} via {dx}");
        }
        // Off-screen pixels clamp to the edge pixels.
        let (dx, dy) = virtual_desk_absolute(-5000, 5000, screen).unwrap();
        assert_eq!((dx, dy * 1080 / 65_536), (0, 1079));
        assert_eq!(virtual_desk_absolute(0, 0, bounds(0.0, 0.0, 0.0, 0.0)), None);
    }

    #[test]
    fn map_key_accepts_printable_unicode_and_punctuation() {
        assert_eq!(map_char('!'), Some(Key::Unicode('!')));
        assert_eq!(map_char('['), Some(Key::Unicode('[')));
        assert_eq!(map_char('é'), Some(Key::Unicode('é')));
        assert_eq!(map_char('\u{7f}'), None);
        assert_eq!(map_key(27), Some(Key::Escape));
    }

    #[test]
    fn modifier_bits_map_to_enigo_keys() {
        assert_eq!(modifier_keys(0b1111, 'c' as u32).collect::<Vec<_>>(), vec![
            Key::Shift,
            Key::Control,
            Key::Alt,
            Key::Meta,
        ]);
    }
}
