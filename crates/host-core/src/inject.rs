use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use protocol::input::InputEvent;

pub fn norm_to_pixel(coord: i32, screen: u32) -> i32 {
    let max_pixel = i64::from(screen).saturating_sub(1);
    ((i64::from(coord).clamp(0, 65_535) * max_pixel + 32_767) / 65_535) as i32
}

pub trait InputInjector {
    fn inject(&mut self, ev: &InputEvent, screen_w: u32, screen_h: u32);
}

impl<T: InputInjector + ?Sized> InputInjector for Box<T> {
    fn inject(&mut self, ev: &InputEvent, screen_w: u32, screen_h: u32) {
        (**self).inject(ev, screen_w, screen_h);
    }
}

pub struct EnigoInjector {
    enigo: Enigo,
}

impl EnigoInjector {
    pub fn new() -> Option<Self> {
        Enigo::new(&Settings::default())
            .ok()
            .map(|enigo| Self { enigo })
    }
}

impl InputInjector for EnigoInjector {
    fn inject(&mut self, ev: &InputEvent, screen_w: u32, screen_h: u32) {
        match *ev {
            InputEvent::MouseMove { x, y } => {
                let _ = self.enigo.move_mouse(
                    norm_to_pixel(x, screen_w),
                    norm_to_pixel(y, screen_h),
                    Coordinate::Abs,
                );
            }
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

#[derive(Default)]
pub struct NoopInjector {
    count: usize,
}

impl NoopInjector {
    pub fn count(&self) -> usize {
        self.count
    }
}

impl InputInjector for NoopInjector {
    fn inject(&mut self, _ev: &InputEvent, _screen_w: u32, _screen_h: u32) {
        self.count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_to_pixel_maps_endpoints() {
        assert_eq!(norm_to_pixel(0, 1920), 0);
        assert_eq!(norm_to_pixel(65535, 1920), 1919);
        assert_eq!(norm_to_pixel(32768, 1000), 500);
    }

    #[test]
    fn noop_injector_records_events() {
        let mut inj = NoopInjector::default();
        inj.inject(
            &protocol::input::InputEvent::MouseButton {
                button: 1,
                down: true,
            },
            1920,
            1080,
        );
        assert_eq!(inj.count(), 1);
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
